//! Saved accounts of the legacy edition: several Discord logins on this Mac, one of them active.
//!
//! Stored as `accounts.json` in the data folder, readable by this user only. It is not
//! encrypted, the same as the single session file it replaces (docs/MACOS_LEGACY.md says so).
//! The main app keeps its own encrypted vault; this one needs no Slint types.
//!
//! An account is identified by its Discord user id once the token has answered once. Until
//! then (an old session file, or a sign-in whose check failed) it has an empty id and is
//! found by its token instead. `key()` is the id when there is one, else the token.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Account {
    /// Discord user id; empty until the token has been checked once.
    #[serde(default)]
    pub id: String,
    /// Display name, as last seen.
    #[serde(default)]
    pub name: String,
    /// Address of the avatar picture as last seen (empty: none).
    #[serde(default)]
    pub avatar: String,
    pub token: String,
}

impl Account {
    /// The name this account is known by inside the app and in the store's `active` field.
    pub fn key(&self) -> String {
        if self.id.is_empty() {
            self.token.clone()
        } else {
            self.id.clone()
        }
    }
}

#[derive(Serialize, Deserialize, Default, Debug, Clone, PartialEq, Eq)]
pub struct Store {
    /// `key()` of the active account; empty when there are none.
    #[serde(default)]
    pub active: String,
    #[serde(default)]
    pub accounts: Vec<Account>,
    /// Where it is saved (not part of the file).
    #[serde(skip)]
    path: PathBuf,
}

impl Store {
    /// Reads `path`. When it does not exist yet but the single session file of earlier
    /// versions does, that token becomes the first account and the session file is removed.
    pub fn load(path: &Path, legacy_session: &Path) -> Store {
        let mut store = match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str::<Store>(&text).unwrap_or_default(),
            Err(_) => Store::default(),
        };
        store.path = path.to_path_buf();
        if store.accounts.is_empty() {
            if let Ok(token) = std::fs::read_to_string(legacy_session) {
                let token = token.trim().to_string();
                if !token.is_empty() {
                    store.accounts.push(Account { id: String::new(), name: String::new(), avatar: String::new(), token });
                    store.active = store.accounts[0].key();
                    store.save();
                    let _ = std::fs::remove_file(legacy_session);
                }
            }
        }
        // An active key that is no longer in the list falls back to the first account.
        if store.active_account().is_none() {
            store.active = store.accounts.first().map(|a| a.key()).unwrap_or_default();
        }
        store
    }

    pub fn active_account(&self) -> Option<&Account> {
        if self.active.is_empty() {
            return None;
        }
        self.accounts.iter().find(|a| a.key() == self.active)
    }

    /// Records a successful check of `token`: the account is added or updated and becomes
    /// the active one. An empty `id` (the check failed) keeps what is known about it.
    pub fn upsert(&mut self, token: &str, id: &str, name: &str, avatar: &str) {
        let position = self
            .accounts
            .iter()
            .position(|a| a.token == token || (!id.is_empty() && a.id == id));
        let index = match position {
            Some(i) => i,
            None => {
                self.accounts.push(Account { id: String::new(), name: String::new(), avatar: String::new(), token: token.to_string() });
                self.accounts.len() - 1
            }
        };
        let account = &mut self.accounts[index];
        account.token = token.to_string();
        if !id.is_empty() {
            account.id = id.to_string();
        }
        if !name.is_empty() {
            account.name = name.to_string();
        }
        if !avatar.is_empty() {
            account.avatar = avatar.to_string();
        }
        self.active = self.accounts[index].key();
        self.save();
    }

    /// Makes the account with this key the active one. Unknown keys are ignored.
    pub fn set_active(&mut self, key: &str) {
        if self.accounts.iter().any(|a| a.key() == key) {
            self.active = key.to_string();
            self.save();
        }
    }

    /// Removes the account with this key. If it was the active one, the first remaining
    /// account becomes active (none when that was the last).
    pub fn remove(&mut self, key: &str) {
        self.accounts.retain(|a| a.key() != key);
        if self.active == key || self.active_account().is_none() {
            self.active = self.accounts.first().map(|a| a.key()).unwrap_or_default();
        }
        self.save();
    }

    /// Writes the file, readable by this user only. A failed write is not fatal: the
    /// accounts are simply not kept across a restart.
    pub fn save(&self) {
        let Ok(text) = serde_json::to_string_pretty(self) else { return };
        if let Some(folder) = self.path.parent() {
            let _ = std::fs::create_dir_all(folder);
        }
        let temporary = self.path.with_extension("json.tmp");
        if std::fs::write(&temporary, text).is_ok() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600));
            }
            let _ = std::fs::rename(&temporary, &self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("litecord-accounts-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        (dir.join("accounts.json"), dir.join("session"))
    }

    #[test]
    fn a_first_sign_in_is_saved_and_active() {
        let (path, session) = scratch("first");
        let mut store = Store::load(&path, &session);
        assert!(store.active_account().is_none());
        store.upsert("tok-a", "111", "Ana", "");
        let reloaded = Store::load(&path, &session);
        assert_eq!(reloaded.accounts.len(), 1);
        assert_eq!(reloaded.active_account().unwrap().name, "Ana");
    }

    #[test]
    fn a_second_account_is_added_and_switching_keeps_both() {
        let (path, session) = scratch("switch");
        let mut store = Store::load(&path, &session);
        store.upsert("tok-a", "111", "Ana", "");
        store.upsert("tok-b", "222", "Bo", "");
        assert_eq!(store.accounts.len(), 2);
        assert_eq!(store.active_account().unwrap().id, "222");
        store.set_active("111");
        assert_eq!(store.active_account().unwrap().token, "tok-a");
        assert_eq!(Store::load(&path, &session).accounts.len(), 2);
    }

    #[test]
    fn the_same_user_again_updates_the_token_not_the_list() {
        let (path, session) = scratch("same");
        let mut store = Store::load(&path, &session);
        store.upsert("tok-a", "111", "Ana", "");
        store.upsert("tok-a2", "111", "Ana B.", "");
        assert_eq!(store.accounts.len(), 1);
        assert_eq!(store.accounts[0].token, "tok-a2");
        assert_eq!(store.accounts[0].name, "Ana B.");
    }

    #[test]
    fn a_failed_check_keeps_what_was_known() {
        let (path, session) = scratch("failed-check");
        let mut store = Store::load(&path, &session);
        store.upsert("tok-a", "111", "Ana", "h");
        store.upsert("tok-a", "", "", "");
        assert_eq!(store.accounts[0].id, "111");
        assert_eq!(store.accounts[0].name, "Ana");
        assert_eq!(store.accounts[0].avatar, "h");
    }

    #[test]
    fn removing_the_active_account_moves_to_the_next_one() {
        let (path, session) = scratch("remove");
        let mut store = Store::load(&path, &session);
        store.upsert("tok-a", "111", "Ana", "");
        store.upsert("tok-b", "222", "Bo", "");
        store.remove("222");
        assert_eq!(store.active_account().unwrap().id, "111");
        store.remove("111");
        assert!(store.accounts.is_empty());
        assert!(store.active_account().is_none());
    }

    #[test]
    fn an_old_session_file_becomes_the_first_account() {
        let (path, session) = scratch("migrate");
        std::fs::write(&session, "tok-old\n").unwrap();
        let store = Store::load(&path, &session);
        assert_eq!(store.accounts.len(), 1);
        assert_eq!(store.accounts[0].token, "tok-old");
        assert_eq!(store.active_account().unwrap().token, "tok-old");
        assert!(!session.exists(), "the old file is removed once migrated");
    }

    #[test]
    fn an_unknown_active_key_falls_back_to_the_first_account() {
        let (path, session) = scratch("fallback");
        std::fs::write(&path, r#"{"active":"gone","accounts":[{"id":"1","token":"t1"},{"id":"2","token":"t2"}]}"#).unwrap();
        let store = Store::load(&path, &session);
        assert_eq!(store.active_account().unwrap().id, "1");
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_only_for_this_user() {
        use std::os::unix::fs::PermissionsExt;
        let (path, session) = scratch("mode");
        let mut store = Store::load(&path, &session);
        store.upsert("tok-a", "111", "Ana", "");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}
