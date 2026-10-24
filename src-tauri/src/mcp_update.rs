//! Agents' MCP server (`slopbar-mcp.exe`) across updates. Agents run it for their whole session, so
//! the installer cannot write it then; an Ultra release's installer first renames it to
//! `slopbar-mcp.old-<time>.exe` (`src-tauri/nsis/ultra-hooks.nsh`), which Windows allows while it
//! runs. Running servers carry on from that copy and new sessions start the new one. The app
//! deletes the copies at startup once nothing runs them.

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

use crate::instance::MCP_EXE;

const OLD_PREFIX: &str = "slopbar-mcp.old-";

pub fn sha256(path: &Path) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher).ok()?;
    Some(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// A server an update set aside, still counted as an agent's server while it runs.
pub fn is_set_aside(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.starts_with(OLD_PREFIX) && name.ends_with(".exe")
}

/// Deletes the copies set aside in `dir`; one an agent's server still runs from cannot be deleted
/// and is kept for a later start. Returns how many are kept.
pub fn prune(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    entries
        .flatten()
        .filter(|e| is_set_aside(&e.file_name().to_string_lossy()))
        .filter(|e| std::fs::remove_file(e.path()).is_err())
        .count()
}

fn install_dir() -> Option<PathBuf> {
    std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf))
}

/// At startup, in the installed app's folder.
#[cfg_attr(not(feature = "desktop"), allow(dead_code))]
pub fn prune_installed() {
    if let Some(dir) = install_dir() {
        prune(&dir);
    }
}

/// For `get_app_info`: the server beside the app and how many old copies still run.
#[cfg_attr(not(feature = "desktop"), allow(dead_code))]
pub fn status() -> serde_json::Value {
    let dir = install_dir();
    let kept = dir.as_ref().and_then(|d| std::fs::read_dir(d).ok()).map_or(0, |entries| {
        entries.flatten().filter(|e| is_set_aside(&e.file_name().to_string_lossy())).count()
    });
    serde_json::json!({
        "sha256": dir.and_then(|d| sha256(&d.join(MCP_EXE))),
        "set_aside": kept,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<_> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        v.sort();
        v
    }

    #[test]
    fn only_copies_set_aside_are_pruned() {
        let dir = TempDir::new();
        for name in [MCP_EXE, "slopbar.exe", "slopbar-mcp.old-20261024-120000-1.exe", "SLOPBAR-MCP.OLD-x.EXE", "slopbar-mcp.old-notes.txt"] {
            std::fs::write(dir.0.join(name), "x").unwrap();
        }
        assert_eq!(prune(&dir.0), 0);
        assert_eq!(names(&dir.0), vec![MCP_EXE, "slopbar-mcp.old-notes.txt", "slopbar.exe"]);
        assert!(sha256(&dir.0.join(MCP_EXE)).is_some_and(|h| h.len() == 64));
    }

    /// What the installer hook relies on: a running server's file can be renamed (not written),
    /// the server keeps running, and the copy is deleted at the first start after it ends.
    #[cfg(windows)]
    #[test]
    fn a_running_server_can_be_set_aside_and_is_deleted_once_it_ends() {
        let dir = TempDir::new();
        let live = dir.0.join(MCP_EXE);
        let system = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        std::fs::copy(Path::new(&system).join(r"System32\cmd.exe"), &live).unwrap();
        let mut running = std::process::Command::new(&live)
            .args(["/c", "ping -n 30 127.0.0.1 >nul"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(std::fs::OpenOptions::new().write(true).open(&live).is_err(), "a running image was writable");
        std::fs::rename(&live, dir.0.join("slopbar-mcp.old-1.exe")).unwrap();
        std::fs::write(&live, "new").unwrap();
        assert_eq!(prune(&dir.0), 1);
        assert!(running.try_wait().unwrap().is_none(), "the running server was stopped");
        running.kill().unwrap();
        running.wait().unwrap();
        // The OS releases the image a moment after the process ends.
        let mut kept = 1;
        for _ in 0..50 {
            kept = prune(&dir.0);
            if kept == 0 { break; }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert_eq!(kept, 0);
        assert_eq!(names(&dir.0), vec![MCP_EXE]);
    }
}
