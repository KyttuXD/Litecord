use std::path::PathBuf;
use tokio::sync::mpsc;
use log::info;
use serde::{Deserialize, Serialize};

const REPO_OWNER: &str = "Ak4ai";
const REPO_NAME: &str = "Litecord";
const CONFIG_FILE: &str = ".litecord_updater_config.json";
#[cfg(target_os = "macos")]
const MACOS_RELEASES_URL: &str = "https://github.com/KyttuXD/Litecord/releases/tag/nightly";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseInfo {
    pub tag_name: String,
    pub version: String,
    pub download_url: String,
    pub release_name: String,
    pub release_body: String,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct UpdaterConfig {
    ignored_version: Option<String>,
}

fn get_config_path() -> PathBuf {
    PathBuf::from(CONFIG_FILE)
}

pub fn is_version_ignored(tag: &str) -> bool {
    let path = get_config_path();
    if path.exists() {
        if let Ok(data) = std::fs::read_to_string(&path) {
            if let Ok(cfg) = serde_json::from_str::<UpdaterConfig>(&data) {
                if let Some(ref ignored) = cfg.ignored_version {
                    return ignored == tag;
                }
            }
        }
    }
    false
}

pub fn save_ignored_version(tag: &str) {
    let cfg = UpdaterConfig {
        ignored_version: Some(tag.to_string()),
    };
    if let Ok(data) = serde_json::to_string_pretty(&cfg) {
        let _ = std::fs::write(get_config_path(), data);
        info!("🏷️ Version {} marked as ignored in {}", tag, CONFIG_FILE);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetailedVersionInfo {
    pub status: VersionComparisonStatus,
    pub local_version: String,
    pub local_hash: String,
    pub is_dirty: bool,
    pub remote_version: String,
    pub remote_tag: String,
    pub release_name: String,
    pub release_body: String,
    pub download_url: String,
    pub formatted_status: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VersionComparisonStatus {
    UpToDate,        // Up to date
    Outdated,        // Outdated
    AheadOfRelease,  // Dev / Ahead of the public release
    Modified,        // Modified locally (dirty)
}

/// Returns the formatted local version string (e.g. "v0.3.8 (0a21d14)" or "v0.3.8 (0a21d14-modified)")
pub fn get_local_version_string() -> String {
    let base_ver = env!("CARGO_PKG_VERSION");
    let git_hash = env!("LITECORD_GIT_HASH");
    let is_dirty = env!("LITECORD_GIT_DIRTY") == "true";

    if is_dirty {
        format!("v{} ({}-modified)", base_ver, git_hash)
    } else {
        format!("v{} ({})", base_ver, git_hash)
    }
}

/// Compares version string a with b (e.g., "0.2.2" > "0.2.1"). Only the numbers count:
/// "1.3.1-beta" is 1.3.1. Reading each part as a whole number turned "1-beta" into 0, so
/// a release that only changed the last number looked like the one already installed.
pub fn is_newer_version(remote_ver: &str, current_ver: &str) -> bool {
    let parse_ver = |v: &str| -> Vec<u64> {
        v.trim_start_matches('v')
            .split('.')
            .map(|p| {
                let digits = p.find(|c: char| !c.is_ascii_digit()).map_or(p, |end| &p[..end]);
                digits.parse::<u64>().unwrap_or(0)
            })
            .collect()
    };

    let remote_parts = parse_ver(remote_ver);
    let current_parts = parse_ver(current_ver);

    let max_len = remote_parts.len().max(current_parts.len());
    for i in 0..max_len {
        let r = remote_parts.get(i).copied().unwrap_or(0);
        let c = current_parts.get(i).copied().unwrap_or(0);
        if r > c {
            return true;
        } else if r < c {
            return false;
        }
    }
    false
}

/// Checks GitHub API for the latest release (respecting ignored versions)
pub async fn check_for_updates() -> Option<ReleaseInfo> {
    let det = check_version_status_detailed(true).await.ok()?;
    if det.status == VersionComparisonStatus::Outdated {
        Some(ReleaseInfo {
            tag_name: det.remote_tag,
            version: det.remote_version,
            download_url: det.download_url,
            release_name: det.release_name,
            release_body: det.release_body,
        })
    } else {
        None
    }
}

/// Manually checks GitHub API for the latest release and returns complete comparison status
pub async fn check_for_updates_manual() -> Result<DetailedVersionInfo, String> {
    check_version_status_detailed(false).await
}

pub async fn check_version_status_detailed(respect_ignored: bool) -> Result<DetailedVersionInfo, String> {
    let local_version = env!("CARGO_PKG_VERSION").to_string();
    let local_hash = env!("LITECORD_GIT_HASH").to_string();
    let is_dirty = env!("LITECORD_GIT_DIRTY") == "true";

    let url = format!("https://api.github.com/repos/{}/{}/releases/latest", REPO_OWNER, REPO_NAME);
    info!("🔍 Comparing local version ({}) with the latest GitHub release...", get_local_version_string());

    let client = reqwest::Client::builder()
        .user_agent("Litecord-App-Updater")
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("Error initializing HTTP client: {:?}", e))?;

    let resp = client.get(&url).send().await
        .map_err(|e| format!("Failed to connect to GitHub: {:?}", e))?;

    if !resp.status().is_success() {
        return Err(format!("GitHub API returned HTTP status {}", resp.status()));
    }

    let json: serde_json::Value = resp.json().await
        .map_err(|e| format!("Error decoding GitHub JSON: {:?}", e))?;

    let tag_name = json["tag_name"].as_str().unwrap_or("v0.0.0").to_string();
    let remote_version = tag_name.trim_start_matches('v').to_string();

    let target_asset_name = if cfg!(target_os = "windows") {
        "Litecord-Setup-x64.exe"
    } else if cfg!(target_os = "macos") {
        "Litecord-macos-x64.zip"
    } else {
        "litecord-linux-x64.tar.gz"
    };

    let mut download_url = String::new();
    if let Some(assets) = json["assets"].as_array() {
        for asset in assets {
            if let Some(name) = asset["name"].as_str() {
                if name == target_asset_name {
                    if let Some(durl) = asset["browser_download_url"].as_str() {
                        download_url = durl.to_string();
                        break;
                    }
                }
            }
        }
    }

    if download_url.is_empty() {
        download_url = format!(
            "https://github.com/{}/{}/releases/download/{}/{}",
            REPO_OWNER, REPO_NAME, tag_name, target_asset_name
        );
    }

    if respect_ignored && is_version_ignored(&tag_name) {
        info!("Remote version {} is marked as ignored by the user.", tag_name);
    }

    let (status, formatted_status) = if is_newer_version(&remote_version, &local_version) {
        (
            VersionComparisonStatus::Outdated,
            format!("Outdated (v{} available on GitHub)", remote_version),
        )
    } else if is_newer_version(&local_version, &remote_version) {
        if is_dirty {
            (
                VersionComparisonStatus::Modified,
                format!("Dev / Modified ({}-modified • release v{})", local_hash, remote_version),
            )
        } else {
            (
                VersionComparisonStatus::AheadOfRelease,
                format!("Up to date / Dev ({} • ahead of release v{})", local_hash, remote_version),
            )
        }
    } else {
        if is_dirty {
            (
                VersionComparisonStatus::Modified,
                format!("Modified ({}-modified • base v{})", local_hash, remote_version),
            )
        } else {
            (
                VersionComparisonStatus::UpToDate,
                format!("Up to date (v{} • {})", local_version, local_hash),
            )
        }
    };

    info!("📊 Versioning status: {}", formatted_status);

    Ok(DetailedVersionInfo {
        status,
        local_version,
        local_hash,
        is_dirty,
        remote_version,
        remote_tag: tag_name.clone(),
        release_name: json["name"].as_str().unwrap_or(&tag_name).to_string(),
        release_body: json["body"].as_str().unwrap_or("").to_string(),
        download_url,
        formatted_status,
    })
}

fn get_secure_update_dir() -> PathBuf {
    #[cfg(unix)]
    {
        if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
            let path = PathBuf::from(runtime_dir).join("litecord_update");
            let _ = std::fs::create_dir_all(&path);
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700));
            return path;
        }
        if let Ok(home) = std::env::var("HOME") {
            let path = PathBuf::from(home).join(".cache").join("litecord").join("update");
            let _ = std::fs::create_dir_all(&path);
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700));
            return path;
        }
    }
    std::env::temp_dir()
}

/// Downloads the release file and launches the updater/installer
#[cfg_attr(target_os = "macos", allow(unreachable_code, unused_variables, unused_mut))]
pub async fn download_and_install_update(
    download_url: String,
    progress_tx: mpsc::Sender<f32>,
) -> Result<(), String> {
    info!("Starting update download: {}", download_url);
    if !download_url.starts_with("https://github.com/Ak4ai/Litecord/")
        && !download_url.starts_with("https://objects.githubusercontent.com/")
        && !download_url.starts_with("https://github-releases.githubusercontent.com/")
    {
        return Err("Update URL rejected for security: unofficial domain.".to_string());
    }

    // macOS: the app is a bundle the user placed somewhere (often /Applications), so it is
    // not replaced in place. The browser downloads the new zip and the user swaps the app.
    #[cfg(target_os = "macos")]
    {
        let _ = &progress_tx;
        // Upstream publishes no macOS file: the macOS builds are the nightlies of the fork.
        let _ = &download_url;
        return match std::process::Command::new("open").arg(MACOS_RELEASES_URL).spawn() {
            Ok(_) => Err("On macOS the new version is downloaded by the browser: replace Litecord.app with the new one and open it again.".to_string()),
            Err(e) => Err(format!("could not open the browser: {:?}", e)),
        };
    }

    let client = reqwest::Client::builder()
        .user_agent("Litecord-App-Updater")
        .build()
        .map_err(|e| format!("Error creating HTTP client: {:?}", e))?;

    let mut response = client.get(&download_url).send().await
        .map_err(|e| format!("Download connection failed: {:?}", e))?;

    if !response.status().is_success() {
        return Err(format!("Download failed with HTTP status {}", response.status()));
    }

    let total_size = response.content_length().unwrap_or(0);
    let temp_dir = get_secure_update_dir();

    if cfg!(target_os = "windows") {
        let installer_path = temp_dir.join("Litecord-Update-Setup.exe");
        let mut file = tokio::fs::File::create(&installer_path).await
            .map_err(|e| format!("Failed to create temporary file: {:?}", e))?;

        let mut downloaded: u64 = 0;

        while let Some(chunk) = response.chunk().await.map_err(|e| format!("Erro no download: {:?}", e))? {
            tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await
                .map_err(|e| format!("Error writing data: {:?}", e))?;

            downloaded += chunk.len() as u64;
            if total_size > 0 {
                let progress = (downloaded as f32 / total_size as f32).clamp(0.0, 1.0);
                let _ = progress_tx.try_send(progress);
            }
        }

        tokio::io::AsyncWriteExt::flush(&mut file).await
            .map_err(|e| format!("Error flushing the file: {:?}", e))?;
        drop(file);

        if total_size > 0 && downloaded != total_size {
            let _ = tokio::fs::remove_file(&installer_path).await;
            return Err(format!("Incomplete download (received {} of {} bytes). The installer was not run.", downloaded, total_size));
        }
        if downloaded < 500_000 {
            let _ = tokio::fs::remove_file(&installer_path).await;
            return Err(format!("Download truncated or corrupted (only {} bytes received).", downloaded));
        }

        let _ = progress_tx.try_send(1.0);
        info!("Download complete and validated ({} bytes)! Running installer: {:?}", downloaded, installer_path);

        // Run installer and terminate current application cleanly
        #[cfg(target_os = "windows")]
        {
            use std::process::Command;
            let installer_str = installer_path.to_string_lossy().to_string();

            // Run detached via cmd with a 1-second delay so this process terminates and unlocks all files
            let res = Command::new("cmd")
                .args([
                    "/c",
                    "timeout",
                    "/t",
                    "1",
                    "/nobreak",
                    ">nul",
                    "&",
                    "start",
                    "",
                    &installer_str,
                    "/SP-",
                    "/SILENT",
                    "/FORCECLOSEAPPLICATIONS",
                    "/RESTARTAPPLICATIONS",
                ])
                .spawn();

            match res {
                Ok(_) => {
                    info!("Installer scheduled successfully. Closing the Litecord process for the update...");
                    unsafe {
                        windows_sys::Win32::System::Threading::TerminateProcess(
                            windows_sys::Win32::System::Threading::GetCurrentProcess(),
                            0,
                        );
                    }
                    std::process::exit(0);
                }
                Err(_e) => {
                    // Fallback direct spawn
                    let _ = Command::new(&installer_path)
                        .args(["/SP-", "/FORCECLOSEAPPLICATIONS", "/RESTARTAPPLICATIONS"])
                        .spawn();
                    std::process::exit(0);
                }
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            Ok(())
        }
    } else {
        // Linux: Download .tar.gz and extract binary to ~/.local/bin/litecord
        let tar_path = temp_dir.join("litecord-update.tar.gz");
        let mut file = tokio::fs::File::create(&tar_path).await
            .map_err(|e| format!("Failed to create temporary file: {:?}", e))?;

        let mut downloaded: u64 = 0;

        while let Some(chunk) = response.chunk().await.map_err(|e| format!("Erro no download: {:?}", e))? {
            tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await
                .map_err(|e| format!("Error writing data: {:?}", e))?;

            downloaded += chunk.len() as u64;
            if total_size > 0 {
                let progress = (downloaded as f32 / total_size as f32).clamp(0.0, 1.0);
                let _ = progress_tx.try_send(progress);
            }
        }

        tokio::io::AsyncWriteExt::flush(&mut file).await
            .map_err(|e| format!("Error flushing the file: {:?}", e))?;
        drop(file);

        if total_size > 0 && downloaded != total_size {
            let _ = tokio::fs::remove_file(&tar_path).await;
            return Err(format!("Incomplete download (received {} of {} bytes). Extraction was cancelled.", downloaded, total_size));
        }
        if downloaded < 500_000 {
            let _ = tokio::fs::remove_file(&tar_path).await;
            return Err(format!("Download truncated or corrupted (only {} bytes received).", downloaded));
        }

        let _ = progress_tx.try_send(1.0);
        info!("Download complete and validated ({} bytes)! Updating the Linux binary...", downloaded);

        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        let install_bin_dir = PathBuf::from(&home).join(".local/bin");
        let _ = std::fs::create_dir_all(&install_bin_dir);

        // Extract tar.gz in temp directory
        let extract_dir = temp_dir.join("litecord_extracted");
        let _ = std::fs::remove_dir_all(&extract_dir);
        let _ = std::fs::create_dir_all(&extract_dir);

        let extract_res = std::process::Command::new("tar")
            .args(["-xzf", tar_path.to_str().unwrap(), "-C", extract_dir.to_str().unwrap()])
            .status();

        if let Ok(status) = extract_res {
            if status.success() {
                let new_bin = extract_dir.join("litecord");
                let target_bin = install_bin_dir.join("litecord");
                if new_bin.exists() {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let _ = std::fs::set_permissions(&new_bin, std::fs::Permissions::from_mode(0o755));
                    }

                    // On Linux/Unix, replacing a running binary in-place with fs::copy fails with ETXTBSY (Text file busy).
                    // We write to a temporary file in the same directory and use atomic rename (or remove_file + copy).
                    let temp_target = install_bin_dir.join(format!(".litecord_update_{}", std::process::id()));
                    let mut updated = false;
                    if let Ok(_) = std::fs::copy(&new_bin, &temp_target) {
                        #[cfg(unix)]
                        {
                            use std::os::unix::fs::PermissionsExt;
                            let _ = std::fs::set_permissions(&temp_target, std::fs::Permissions::from_mode(0o755));
                        }
                        if std::fs::rename(&temp_target, &target_bin).is_ok() {
                            updated = true;
                        }
                    }

                    if !updated {
                        // Fallback: unlink running destination first, then copy
                        let _ = std::fs::remove_file(&target_bin);
                        if let Ok(_) = std::fs::copy(&new_bin, &target_bin) {
                            #[cfg(unix)]
                            {
                                use std::os::unix::fs::PermissionsExt;
                                let _ = std::fs::set_permissions(&target_bin, std::fs::Permissions::from_mode(0o755));
                            }
                        }
                    }

                    // Also update the currently running executable location if it's outside ~/.local/bin
                    let mut bin_to_spawn = target_bin.clone();
                    if let Ok(curr_exe) = std::env::current_exe() {
                        if curr_exe.exists() {
                            if curr_exe != target_bin {
                                if let Some(parent) = curr_exe.parent() {
                                    let curr_temp = parent.join(format!(".litecord_currexe_update_{}", std::process::id()));
                                    if let Ok(_) = std::fs::copy(&new_bin, &curr_temp) {
                                        #[cfg(unix)]
                                        {
                                            use std::os::unix::fs::PermissionsExt;
                                            let _ = std::fs::set_permissions(&curr_temp, std::fs::Permissions::from_mode(0o755));
                                        }
                                        if std::fs::rename(&curr_temp, &curr_exe).is_ok() {
                                            bin_to_spawn = curr_exe;
                                        }
                                    }
                                }
                            } else {
                                bin_to_spawn = curr_exe;
                            }
                        }
                    }

                    // Update desktop icon if packaged in archive
                    let new_icon = extract_dir.join("assets/app_icon.png");
                    if new_icon.exists() {
                        let icon_512 = PathBuf::from(&home).join(".local/share/icons/hicolor/512x512/apps/litecord.png");
                        let icon_256 = PathBuf::from(&home).join(".local/share/icons/hicolor/256x256/apps/litecord.png");
                        let icon_pix = PathBuf::from(&home).join(".local/share/pixmaps/litecord.png");
                        let _ = std::fs::create_dir_all(icon_512.parent().unwrap());
                        let _ = std::fs::create_dir_all(icon_256.parent().unwrap());
                        let _ = std::fs::create_dir_all(icon_pix.parent().unwrap());
                        let _ = std::fs::copy(&new_icon, &icon_512);
                        let _ = std::fs::copy(&new_icon, &icon_256);
                        let _ = std::fs::copy(&new_icon, &icon_pix);
                    }

                    info!("✅ Litecord updated successfully at {:?}! Restarting...", bin_to_spawn);
                    let _ = std::process::Command::new(&bin_to_spawn).spawn();
                    std::process::exit(0);
                }
            }
        }
        Err("Failed to extract and install the update on Linux.".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::is_newer_version;

    #[test]
    fn a_new_last_number_with_a_suffix_is_newer() {
        assert!(is_newer_version("1.3.1-beta", "1.3.0-beta"));
        assert!(is_newer_version("v1.3.1-beta", "1.3.0-beta"));
        assert!(is_newer_version("1.4.0-beta", "1.3.1-beta"));
        assert!(!is_newer_version("1.3.1-beta", "1.3.1-beta"));
        assert!(!is_newer_version("1.3.0-beta", "1.3.1-beta"));
        assert!(!is_newer_version("1.3.1", "1.3.1-beta"));
    }
}
