//! Isolated CLI process environment and recovery of abandoned sign-in directories.
use crate::{accounts::Provider, sources::Source};
use std::path::Path;
use std::process::{Child, Command};
use std::time::{Duration, SystemTime};

pub fn blocked_environment(name: &str) -> bool {
    let name = name.to_ascii_uppercase();
    ["CLAUDE_", "ANTHROPIC_", "CODEX_", "OPENAI_"]
        .iter()
        .any(|p| name.starts_with(p))
        || matches!(
            name.as_str(),
            "CLAUDECODE" | "USE_LOCAL_OAUTH" | "USE_STAGING_OAUTH"
        )
}

pub fn shell_script(provider: Provider, native: &str, action: &str, visible: bool) -> String {
    let native = native.replace('\'', "'\\''");
    // Expand this allowlist AFTER the login profile runs. env -i also drops exported
    // functions, BASH_ENV, provider overrides, and settings re-exported by that profile.
    let keep = [
        "PATH",
        "HOME",
        "USER",
        "LOGNAME",
        "TERM",
        "LANG",
        "LC_ALL",
        "DISPLAY",
        "WAYLAND_DISPLAY",
        "XDG_RUNTIME_DIR",
        "DBUS_SESSION_BUS_ADDRESS",
        "WSL_INTEROP",
        "WSL_DISTRO_NAME",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "NO_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
        "no_proxy",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
        "NODE_EXTRA_CA_CERTS",
    ];
    let env = keep
        .iter()
        .map(|n| format!("{n}=\"${{{n}-}}\""))
        .collect::<Vec<_>>()
        .join(" ");
    let config = match provider {
        Provider::Claude => {
            format!("CLAUDE_CONFIG_DIR='{native}' CLAUDE_SECURESTORAGE_CONFIG_DIR='{native}'")
        }
        Provider::Codex => format!("CODEX_HOME='{native}'"),
    };
    let pause = if visible {
        "if [ \"$slopbar_login_status\" -ne 0 ]; then read -r -p 'Sign-in failed. Press Enter to close.'; fi; "
    } else {
        ""
    };
    format!("chmod 700 -- '{native}' && env -i {env} {config} {action}; slopbar_login_status=$?; {pause}exit \"$slopbar_login_status\"")
}

pub fn command(
    provider: Provider,
    source: &Source,
    dir: &Path,
    native: &str,
    action: &str,
    visible: bool,
) -> Command {
    #[allow(unused_mut)]
    let mut command = match &source.distro {
        Some(distro) => {
            let mut c = crate::sources::command("wsl.exe");
            c.args([
                "-d",
                distro,
                "--",
                "bash",
                "-lic",
                &shell_script(provider, native, action, visible),
            ]);
            c
        }
        None => {
            let mut c = crate::sources::command("cmd.exe");
            // /d disables cmd AutoRun, which can restore authentication overrides.
            c.arg("/d").arg("/c");
            if visible {
                c.arg(dir.join("login.cmd"));
            } else {
                c.arg(action);
            }
            for (name, _) in std::env::vars_os() {
                if blocked_environment(&name.to_string_lossy()) {
                    c.env_remove(name);
                }
            }
            match provider {
                Provider::Claude => {
                    c.env("CLAUDE_CONFIG_DIR", dir)
                        .env("CLAUDE_SECURESTORAGE_CONFIG_DIR", dir);
                }
                Provider::Codex => {
                    c.env("CODEX_HOME", dir);
                }
            }
            if let Some(home) = dirs::home_dir() {
                c.current_dir(home);
            }
            c
        }
    };
    if visible {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0000_0010); // CREATE_NEW_CONSOLE
        }
    }
    command
}

pub fn prepare(dir: &Path, provider: Provider, action: &str) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    std::fs::write(
        dir.join("slopbar-owner.json"),
        serde_json::to_vec(&provider)?,
    )?;
    // `call` supports CLIs installed as .cmd launchers. Keep the original exit code
    // after pause so a failed login can never be imported as a successful one.
    std::fs::write(dir.join("login.cmd"), format!("@echo off\r\ncall {action}\r\nset \"slopbar_login_status=%errorlevel%\"\r\nif not \"%slopbar_login_status%\"==\"0\" pause\r\nexit /b %slopbar_login_status%\r\n"))
}

/// A hidden logout is needed if the CLI wrote to its isolated OS credential store.
/// Always use the same sanitized environment and config directory as the login.
pub fn logout(provider: Provider, source: &Source, dir: &Path, native: &str) {
    let action = match provider {
        Provider::Claude => "claude auth logout",
        Provider::Codex => "codex logout",
    };
    let Ok(mut child) = command(provider, source, dir, native, action, false)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    else {
        return;
    };
    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) if started.elapsed() < Duration::from_secs(6) => {
                std::thread::sleep(Duration::from_millis(50))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return;
            }
        }
    }
}

fn abandoned(name: &str, now: SystemTime) -> bool {
    let Some(rest) = name.strip_prefix(".slopbar-login-") else {
        return false;
    };
    let fields: Vec<_> = rest.split('-').collect();
    if fields.len() != 3
        || fields
            .iter()
            .any(|v| v.is_empty() || !v.bytes().all(|c| c.is_ascii_digit()))
    {
        return false;
    }
    let Ok(millis) = fields[1].parse::<u64>() else {
        return false;
    };
    SystemTime::UNIX_EPOCH
        .checked_add(Duration::from_millis(millis))
        .and_then(|t| now.duration_since(t).ok())
        .is_some_and(|age| age >= Duration::from_secs(15 * 60))
}

pub fn cleanup(sources: &[Source]) {
    cleanup_with(sources, logout);
}

fn cleanup_with(sources: &[Source], mut logout: impl FnMut(Provider, &Source, &Path, &str)) {
    for source in sources {
        let Ok(entries) = std::fs::read_dir(&source.home) else {
            continue;
        };
        for entry in entries.flatten() {
            if !abandoned(&entry.file_name().to_string_lossy(), SystemTime::now()) {
                continue;
            }
            let path = entry.path();
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if !meta.is_dir() || meta.file_type().is_symlink() {
                continue;
            }
            let marker = path.join("slopbar-owner.json");
            // Only our marked directories are eligible. Never follow a marker symlink.
            let provider = if std::fs::symlink_metadata(&marker)
                .is_ok_and(|m| m.is_file() && !m.file_type().is_symlink())
            {
                std::fs::read(&marker)
                    .ok()
                    .and_then(|b| serde_json::from_slice::<Provider>(&b).ok())
            } else if !marker.exists() {
                // Recover folders from the first version, which had no ownership marker.
                // Require the exact generated name AND a regular CLI credentials file.
                [
                    (".credentials.json", "claudeAiOauth", Provider::Claude),
                    ("auth.json", "tokens", Provider::Codex),
                ]
                .into_iter()
                .find_map(|(file, field, provider)| {
                    let file = path.join(file);
                    if !std::fs::symlink_metadata(&file)
                        .is_ok_and(|m| m.is_file() && !m.file_type().is_symlink())
                    {
                        return None;
                    }
                    let data: serde_json::Value =
                        serde_json::from_slice(&std::fs::read(file).ok()?).ok()?;
                    data[field].is_object().then_some(provider)
                })
            } else {
                None
            };
            if let Some(provider) = provider {
                let native = format!(
                    "{}/{}",
                    source.native_home,
                    entry.file_name().to_string_lossy()
                );
                logout(provider, source, &path, &native);
                let _ = std::fs::remove_dir_all(&path);
            }
        }
    }
}

// Closing the Windows process closes this handle and kills the entire sign-in
// process tree, even on an updater restart or an abrupt app exit.
#[cfg(windows)]
pub struct ProcessTree(windows_sys::Win32::Foundation::HANDLE);
#[cfg(windows)]
unsafe impl Send for ProcessTree {}
#[cfg(windows)]
impl ProcessTree {
    pub fn attach(child: &Child) -> std::io::Result<Self> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::JobObjects::*;
        let tree = Self(unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) });
        if tree.0.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let ok = unsafe {
            SetInformationJobObject(
                tree.0,
                JobObjectExtendedLimitInformation,
                &info as *const _ as _,
                std::mem::size_of_val(&info) as u32,
            ) != 0
                && AssignProcessToJobObject(tree.0, child.as_raw_handle() as _) != 0
        };
        if !ok {
            return Err(std::io::Error::last_os_error());
        }
        Ok(tree)
    }
}
#[cfg(windows)]
impl Drop for ProcessTree {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}
#[cfg(not(windows))]
pub struct ProcessTree;
#[cfg(not(windows))]
impl Drop for ProcessTree {
    fn drop(&mut self) {}
}
#[cfg(not(windows))]
impl ProcessTree {
    pub fn attach(_: &Child) -> std::io::Result<Self> {
        Ok(Self)
    }
}

#[cfg(test)]
mod tests;
