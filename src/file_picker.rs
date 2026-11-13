//! A native "choose a file" dialog without a GUI library: the system's own tool for it.
//!
//! - macOS: `osascript` with `choose file` (Standard Additions, OS X 10.5 and later).
//! - Windows: PowerShell's `OpenFileDialog`.
//! - Linux: `zenity`, or `kdialog` first on KDE; whichever is installed.
//!
//! The call blocks until the dialog is closed, so run it on a thread, never on the UI
//! thread. It is shared with the legacy edition (legacy/app/src/lib.rs includes this file).

use std::path::PathBuf;
use std::process::Command;

/// Chooses one file. `Ok(None)` when the dialog was cancelled. `Err` carries a sentence
/// for the person, for example when no dialog tool is installed.
pub fn pick_file(title: &str) -> Result<Option<PathBuf>, String> {
    // The title goes inside quotes in the scripts below, so quotes are removed from it.
    let title: String = title.chars().filter(|c| *c != '"' && *c != '\'' && !c.is_control()).collect();
    #[cfg(target_os = "macos")]
    {
        pick_macos(&title)
    }
    #[cfg(windows)]
    {
        pick_windows(&title)
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        pick_linux(&title)
    }
}

/// The first line of a tool's output as a path; empty output means nothing was chosen.
fn path_from(stdout: &[u8]) -> Option<PathBuf> {
    let text = String::from_utf8_lossy(stdout);
    let line = text.lines().next().unwrap_or("").trim_end();
    if line.is_empty() {
        None
    } else {
        Some(PathBuf::from(line))
    }
}

#[cfg(target_os = "macos")]
fn pick_macos(title: &str) -> Result<Option<PathBuf>, String> {
    let script = format!("POSIX path of (choose file with prompt \"{title}\")");
    let output = Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .output()
        .map_err(|e| format!("The file dialog is not available: {e}"))?;
    if output.status.success() {
        return Ok(path_from(&output.stdout));
    }
    // "User canceled." comes with error number -128.
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("-128") || stderr.to_lowercase().contains("cancel") {
        Ok(None)
    } else {
        Err(format!("The file dialog failed: {}", stderr.trim()))
    }
}

#[cfg(windows)]
fn pick_windows(title: &str) -> Result<Option<PathBuf>, String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let script = format!(
        "Add-Type -AssemblyName System.Windows.Forms; $d = New-Object System.Windows.Forms.OpenFileDialog; \
         $d.Title = '{title}'; if ($d.ShowDialog() -eq 'OK') {{ Write-Output $d.FileName }}"
    );
    let output = Command::new("powershell")
        .args(["-NoProfile", "-STA", "-Command", &script])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("The file dialog is not available: {e}"))?;
    if !output.status.success() {
        return Err(format!("The file dialog failed: {}", String::from_utf8_lossy(&output.stderr).trim()));
    }
    Ok(path_from(&output.stdout))
}

#[cfg(not(any(target_os = "macos", windows)))]
fn pick_linux(title: &str) -> Result<Option<PathBuf>, String> {
    let kde = std::env::var("XDG_CURRENT_DESKTOP").map(|d| d.to_lowercase().contains("kde")).unwrap_or(false);
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
    let zenity: (&str, Vec<String>) = ("zenity", vec!["--file-selection".into(), format!("--title={title}")]);
    let kdialog: (&str, Vec<String>) = ("kdialog", vec!["--title".into(), title.into(), "--getopenfilename".into(), home]);
    let tools = if kde { [kdialog, zenity] } else { [zenity, kdialog] };
    for (program, args) in tools {
        match Command::new(program).args(&args).output() {
            Ok(output) if output.status.success() => return Ok(path_from(&output.stdout)),
            // Both tools exit with 1 when the dialog is cancelled.
            Ok(output) if output.status.code() == Some(1) => return Ok(None),
            Ok(output) => {
                return Err(format!("The file dialog failed: {}", String::from_utf8_lossy(&output.stderr).trim()));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(format!("The file dialog failed: {e}")),
        }
    }
    Err("No file dialog found: install zenity or kdialog, or type /upload /path/to/file.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_line_is_the_path_and_empty_output_is_nothing() {
        assert_eq!(path_from(b"/tmp/a b.pdf\n"), Some(PathBuf::from("/tmp/a b.pdf")));
        assert_eq!(path_from(b"\n"), None);
        assert_eq!(path_from(b""), None);
    }

    #[test]
    fn quotes_are_removed_from_the_title() {
        let cleaned: String = "Choose \"a\" 'file'".chars().filter(|c| *c != '"' && *c != '\'' && !c.is_control()).collect();
        assert_eq!(cleaned, "Choose a file");
    }
}
