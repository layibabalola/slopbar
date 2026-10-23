//! Where a CLI sign-in's web page opens. By default each sign-in, or each email's run of sign-ins
//! in "Sign in all accounts", gets a brand-new empty browser profile (a [`Session`]: a folder under
//! `signin-sessions` in the app's local data folder): no Google account from an earlier sign-in is
//! offered, Google is asked once for all of an email's sign-ins, and once they end the browser is
//! closed and the folder deleted. The user completes every page; SlopBar only chooses the window.
//!
//! Profiles used to be kept per email (`signin-profiles`), and they collected stale Google
//! sessions; [`cleanup`] deletes them at startup.
//!
//! The CLIs open their page themselves, so SlopBar steers them: Claude Code runs `BROWSER` with
//! the URL on Windows, so `BROWSER` points at `slopbar.exe` with [`TARGET_ENV`] saying where to
//! open it ([`launch_from_env`] handles that start before anything else). Codex ignores `BROWSER`
//! on Windows; its app server hands the URL back instead (see `accounts`).

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Set on a CLI sign-in that `BROWSER` points at SlopBar: `profile:<folder>` or `private`.
pub const TARGET_ENV: &str = "SLOPBAR_SIGNIN_TARGET";

/// The user's choice of window (Settings > Accounts).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Window {
    /// A fresh browser profile for each email's sign-ins, deleted once they end. (The name is
    /// kept from when profiles were kept per email, so saved settings still read.)
    #[default]
    Profile,
    /// A private (InPrivate / Incognito) window: nothing is kept, so Google asks every time the
    /// private session was closed.
    Private,
    /// Whatever the CLI opens: the default browser, as before.
    System,
}

/// Where one sign-in's page opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Profile(PathBuf),
    Private,
}

impl Target {
    /// The window for a sign-in. A profile is `session`'s; without one (no data folder), or with
    /// `System`, the CLI opens its page as it always did.
    pub fn choose(window: Window, session: Option<&Session>) -> Option<Self> {
        match window {
            Window::System => None,
            Window::Private => Some(Self::Private),
            Window::Profile => session.map(Session::target),
        }
    }

    fn encode(&self) -> OsString {
        match self {
            Self::Private => "private".into(),
            Self::Profile(dir) => {
                let mut s = OsString::from("profile:");
                s.push(dir);
                s
            }
        }
    }

    fn decode(value: &std::ffi::OsStr) -> Option<Self> {
        let value = value.to_str()?;
        if value == "private" {
            return Some(Self::Private);
        }
        Some(Self::Profile(PathBuf::from(value.strip_prefix("profile:")?)))
    }
}

/// Where the fresh profiles live while their sign-ins run: they hold provider and Google
/// cookies, so outside the roaming profile and never in a backup.
pub fn sessions_root() -> Option<PathBuf> {
    Some(crate::settings::local_dir()?.join("signin-sessions"))
}

/// The profiles kept per email by earlier versions; [`cleanup`] deletes them.
fn legacy_root() -> Option<PathBuf> {
    Some(crate::settings::local_dir()?.join("signin-profiles"))
}

/// Profiles whose sign-ins are running: never deleted from under them by [`cleanup`].
static ACTIVE: Mutex<Option<HashSet<PathBuf>>> = Mutex::new(None);

fn set_active(dir: &Path, active: bool) {
    let mut set = ACTIVE.lock().unwrap_or_else(|e| e.into_inner());
    let set = set.get_or_insert_with(HashSet::new);
    if active { set.insert(dir.to_owned()); } else { set.remove(dir); }
}

fn is_active(dir: &Path) -> bool {
    ACTIVE.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|s| s.contains(dir))
}

/// A brand-new empty browser profile for one email's sign-ins (or one sign-in). Every sign-in
/// given its [`target`](Session::target) opens in the same browser, so Google is asked once; when
/// the session is dropped that browser is closed and the folder overwritten and deleted.
#[derive(Debug)]
pub struct Session {
    dir: PathBuf,
}

impl Session {
    /// A new profile folder under `root`, named at random so no two sessions ever share one.
    pub fn create_in(root: &Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(root)?;
        let dir = root.join(uuid::Uuid::new_v4().simple().to_string());
        std::fs::create_dir(&dir)?;
        set_active(&dir, true);
        Ok(Self { dir })
    }

    /// A new profile in [`sessions_root`].
    pub fn create() -> Option<Self> {
        Self::create_in(&sessions_root()?).ok()
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn target(&self) -> Target {
        Target::Profile(self.dir.clone())
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        close_browser(&self.dir);
        let _ = remove_profile(&self.dir);
        set_active(&self.dir, false);
    }
}

/// Closes every running session's browser and deletes its profile, as SlopBar exits.
pub fn close_all() {
    let dirs: Vec<PathBuf> = ACTIVE.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|s| s.iter().cloned().collect()).unwrap_or_default();
    for dir in dirs {
        close_browser(&dir);
        let _ = remove_profile(&dir);
    }
}

/// Closes the browser running `dir` as its profile, if one is: its main process (see
/// [`browser_pid`]) and the ones it started are ended. The profile is about to be deleted, so
/// there is nothing to save.
#[cfg(windows)]
pub fn close_browser(dir: &Path) {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
    use windows_sys::Win32::System::Threading::{OpenProcess, TerminateProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE};
    let Some(pid) = browser_pid(dir) else { return };
    // The browser process and its children (renderers, GPU), found before it ends.
    let mut pids = vec![pid];
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap != INVALID_HANDLE_VALUE {
            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            let mut more = Process32FirstW(snap, &mut entry) != 0;
            while more {
                if entry.th32ParentProcessID == pid && entry.th32ProcessID != pid {
                    pids.push(entry.th32ProcessID);
                }
                more = Process32NextW(snap, &mut entry) != 0;
            }
            CloseHandle(snap);
        }
    }
    for p in pids {
        unsafe {
            let h = OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, 0, p);
            if !h.is_null() {
                TerminateProcess(h, 0);
                WaitForSingleObject(h, 3000);
                CloseHandle(h);
            }
        }
    }
}

#[cfg(not(windows))]
pub fn close_browser(_: &Path) {}

/// Whether a browser runs `dir` as its profile (see [`close_browser`]).
#[cfg(windows)]
pub fn browser_open(dir: &Path) -> bool {
    browser_pid(dir).is_some()
}

#[cfg(not(windows))]
pub fn browser_open(_: &Path) -> bool {
    false
}

/// The main process of the Chromium browser whose profile is `dir`: it names its hidden
/// `Chrome_MessageWindow` after the folder.
#[cfg(windows)]
fn browser_pid(dir: &Path) -> Option<u32> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowExW, GetWindowThreadProcessId, HWND_MESSAGE};
    let class: Vec<u16> = "Chrome_MessageWindow".encode_utf16().chain([0]).collect();
    let title: Vec<u16> = dir.as_os_str().encode_wide().chain([0]).collect();
    let mut pid = 0u32;
    unsafe {
        let hwnd = FindWindowExW(HWND_MESSAGE, std::ptr::null_mut(), class.as_ptr(), title.as_ptr());
        if hwnd.is_null() {
            return None;
        }
        GetWindowThreadProcessId(hwnd, &mut pid);
    }
    (pid != 0).then_some(pid)
}

/// Deletes a profile folder, first overwriting each file with zeros so its cookies do not linger
/// in freed disk space. Files a closing browser still holds are retried for a few seconds.
pub fn remove_profile(dir: &Path) -> std::io::Result<()> {
    let mut last = Ok(());
    for attempt in 0..20 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(250));
        }
        scrub(dir);
        last = match std::fs::remove_dir_all(dir) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            other => other,
        };
        if last.is_ok() {
            break;
        }
    }
    last
}

/// Overwrites every file under `dir` with zeros, never following a link out of it.
fn scrub(dir: &Path) {
    use std::io::Write;
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let zeros = [0u8; 64 * 1024];
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else { continue };
        let path = entry.path();
        if kind.is_symlink() {
            continue;
        } else if kind.is_dir() {
            scrub(&path);
        } else if let (Ok(len), Ok(mut f)) = (entry.metadata().map(|m| m.len()), std::fs::OpenOptions::new().write(true).open(&path)) {
            let mut left = len;
            while left > 0 {
                let n = left.min(zeros.len() as u64) as usize;
                if f.write_all(&zeros[..n]).is_err() {
                    break;
                }
                left -= n as u64;
            }
            let _ = f.sync_all();
        }
    }
}

/// An email safe to hand to a CLI on its command line (Claude's `--email`). The sign-in runs
/// through a `cmd` script, so anything beyond the plain address characters is not prefilled.
pub fn prefill_email(email: Option<&str>) -> Option<&str> {
    let email = email?.trim();
    let (local, domain) = email.split_once('@')?;
    let plain = |s: &str, extra: &[u8]| !s.is_empty() && s.bytes().all(|c| c.is_ascii_alphanumeric() || extra.contains(&c));
    (email.len() <= 254 && plain(local, b"._+-") && plain(domain, b".-") && domain.contains('.')
        && !domain.starts_with(['.', '-']) && !local.starts_with('-'))
        .then_some(email)
}

/// Only the providers' own sign-in pages are opened, over HTTPS.
pub fn allowed_url(url: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(url) else { return false };
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() || url.port().is_some() {
        return false;
    }
    let Some(host) = url.host_str() else { return false };
    let host = host.to_ascii_lowercase();
    ["claude.ai", "claude.com", "anthropic.com", "openai.com", "chatgpt.com"]
        .iter()
        .any(|d| host == *d || host.ends_with(&format!(".{d}")))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Chrome,
    Edge,
}

/// A browser that takes `--user-data-dir`: Chrome when installed (its first run is quiet with
/// `--no-first-run`), else Edge, which every Windows 11 has.
pub fn find_browser() -> Option<(Kind, PathBuf)> {
    let env = |n: &str| std::env::var_os(n).map(PathBuf::from);
    let mut candidates = Vec::new();
    for base in [env("ProgramFiles"), env("ProgramFiles(x86)"), env("LOCALAPPDATA")].into_iter().flatten() {
        candidates.push((Kind::Chrome, base.join(r"Google\Chrome\Application\chrome.exe")));
    }
    for base in [env("ProgramFiles(x86)"), env("ProgramFiles")].into_iter().flatten() {
        candidates.push((Kind::Edge, base.join(r"Microsoft\Edge\Application\msedge.exe")));
    }
    candidates.into_iter().find(|(_, p)| p.is_file())
}

/// The browser's arguments for one page. The URL goes last and is an https URL, so it can never
/// be read as a switch.
pub fn browser_args(kind: Kind, target: &Target, url: &str) -> Vec<OsString> {
    let mut args: Vec<OsString> = Vec::new();
    match target {
        Target::Profile(dir) => {
            let mut a = OsString::from("--user-data-dir=");
            a.push(dir);
            args.push(a);
            // A new profile: no welcome pages, browser sign-in or sync offers, extensions or
            // default-browser question in the way of the provider's page.
            for flag in ["--no-first-run", "--no-default-browser-check", "--disable-sync", "--disable-extensions", "--disable-default-apps"] {
                args.push(flag.into());
            }
        }
        Target::Private => args.push(match kind { Kind::Chrome => "--incognito", Kind::Edge => "--inprivate" }.into()),
    }
    args.push("--new-window".into());
    args.push(url.into());
    args
}

/// Opens a provider's sign-in page in `target`. A profile must be a session's folder that exists.
pub fn open(url: &str, target: &Target) -> Result<(), String> {
    if !allowed_url(url) {
        return Err("signin_url_rejected".into());
    }
    if let Target::Profile(dir) = target {
        let root = sessions_root().ok_or("signin_profile_storage")?;
        if dir.parent() != Some(root.as_path()) || !dir.is_dir() {
            return Err("signin_profile_storage".into());
        }
    }
    let (kind, exe) = find_browser().ok_or("signin_browser_missing")?;
    let spawn = |breakaway: bool| {
        let mut c = std::process::Command::new(&exe);
        c.args(browser_args(kind, target, url))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        // A Claude sign-in's processes share a kill-on-close job (see `login::ProcessTree`) that
        // lets this one break away: the browser outlives the sign-in, so finishing it does not
        // close the user's window. Started outside such a job, it just starts.
        #[cfg(windows)]
        if breakaway {
            use std::os::windows::process::CommandExt;
            c.creation_flags(windows_sys::Win32::System::Threading::CREATE_BREAKAWAY_FROM_JOB);
        }
        #[cfg(not(windows))]
        let _ = breakaway;
        c.spawn().map(|_| ())
    };
    spawn(true).or_else(|_| spawn(false)).map_err(|_| "signin_browser_missing".into())
}

/// The environment that makes a Claude sign-in open its page in `target`: `BROWSER` names this
/// executable (by its short path when the long one has spaces, in case the CLI runs it through a
/// shell), and [`TARGET_ENV`] where to open it.
pub fn claude_browser_env(target: &Target) -> Option<[(OsString, OsString); 2]> {
    let exe = std::env::current_exe().ok()?;
    let exe = short_path(&exe).unwrap_or(exe);
    Some([("BROWSER".into(), exe.into_os_string()), (TARGET_ENV.into(), target.encode())])
}

#[cfg(windows)]
fn short_path(path: &Path) -> Option<PathBuf> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    if !path.to_string_lossy().contains(' ') {
        return None;
    }
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let mut buf = vec![0u16; 1024];
    let n = unsafe { windows_sys::Win32::Storage::FileSystem::GetShortPathNameW(wide.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) } as usize;
    if n == 0 || n >= buf.len() {
        return None;
    }
    let short = PathBuf::from(OsString::from_wide(&buf[..n]));
    (!short.to_string_lossy().contains(' ')).then_some(short)
}

#[cfg(not(windows))]
fn short_path(_: &Path) -> Option<PathBuf> {
    None
}

/// `slopbar.exe <url>` started by a CLI through `BROWSER`, with [`TARGET_ENV`] set: opens the
/// page and returns the exit code, before any of the app starts. `None`: not such a start.
pub fn launch_from_env(args: &[OsString]) -> Option<i32> {
    let target = std::env::var_os(TARGET_ENV)?;
    if args.len() != 2 {
        return Some(2);
    }
    let Some(target) = Target::decode(&target) else { return Some(2) };
    let url = args[1].to_str()?;
    Some(if open(url, &target).is_ok() { 0 } else { 1 })
}

/// Profiles nothing is using under `legacy` (the per-email ones earlier versions kept) and
/// `sessions` (ones left by a SlopBar that ended mid-sign-in).
fn stale_in(legacy: Option<&Path>, sessions: Option<&Path>) -> Vec<PathBuf> {
    let dirs = |root: &Path| -> Vec<PathBuf> {
        std::fs::read_dir(root).map(|e| e.flatten().map(|e| e.path())
            .filter(|p| std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())).collect())
            .unwrap_or_default()
    };
    let mut found: Vec<PathBuf> = legacy.map(dirs).unwrap_or_default();
    found.extend(sessions.map(dirs).unwrap_or_default().into_iter().filter(|d| !is_active(d)));
    found
}

fn cleanup_in(legacy: Option<&Path>, sessions: Option<&Path>) -> Result<usize, String> {
    let (mut removed, mut failed) = (0, 0);
    for dir in stale_in(legacy, sessions) {
        close_browser(&dir);
        if remove_profile(&dir).is_ok() { removed += 1 } else { failed += 1 }
    }
    if failed > 0 {
        return Err("signin_profiles_busy".into());
    }
    if let Some(legacy) = legacy {
        let _ = std::fs::remove_dir(legacy);
    }
    Ok(removed)
}

/// Closes and deletes every profile nothing is using: runs at startup, so the profiles kept per
/// email go on the first start of this version, and from Settings ("Forget sign-in profiles").
/// Returns how many were removed, or `signin_profiles_busy` when one could not be.
pub fn cleanup() -> Result<usize, String> {
    cleanup_in(legacy_root().as_deref(), sessions_root().as_deref())
}

/// How many profiles [`cleanup`] would remove, for Settings.
pub fn count() -> usize {
    stale_in(legacy_root().as_deref(), sessions_root().as_deref()).len()
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::test_support::TempDir;

    /// A profile as a browser leaves it: cookies in a nested folder.
    fn fill(dir: &Path) -> PathBuf {
        let cookies = dir.join("Default").join("Network").join("Cookies");
        std::fs::create_dir_all(cookies.parent().unwrap()).unwrap();
        std::fs::write(&cookies, b"session=synthetic-cookie-for-a-test").unwrap();
        cookies
    }

    #[test]
    fn each_session_is_a_new_empty_profile_its_sign_ins_share() {
        let root = TempDir::new();
        let a = Session::create_in(&root.0).unwrap();
        let b = Session::create_in(&root.0).unwrap();
        assert_ne!(a.dir(), b.dir(), "the next email gets a profile of its own");
        for s in [&a, &b] {
            assert_eq!(s.dir().parent(), Some(root.0.as_path()));
            assert_eq!(std::fs::read_dir(s.dir()).unwrap().count(), 0, "nothing from an earlier sign-in");
            assert!(is_active(s.dir()));
        }
        assert_eq!(a.target(), a.target(), "an email's Claude and Codex sign-ins open in the same profile");
        assert_eq!(Target::choose(Window::Profile, Some(&a)), Some(a.target()));
        assert_eq!(Target::choose(Window::Profile, None), None, "no profile: the CLI's own browser");
        assert_eq!(Target::choose(Window::Private, Some(&a)), Some(Target::Private));
    }

    #[test]
    fn a_session_is_deleted_however_its_sign_ins_end() {
        let root = TempDir::new();
        // Finished: the email's last sign-in completed.
        let done = Session::create_in(&root.0).unwrap();
        let dir = done.dir().to_owned();
        fill(&dir);
        drop(done);
        assert!(!dir.exists() && !is_active(&dir));
        // Failed: a sign-in returned an error and the group ended early.
        let failed = |root: &Path| -> Result<PathBuf, (String, PathBuf)> {
            let s = Session::create_in(root).unwrap();
            fill(s.dir());
            Err(("monitor_wrong_account".to_string(), s.dir().to_owned()))
        };
        let (_, dir) = failed(&root.0).unwrap_err();
        assert!(!dir.exists());
        // Stopped: the running sign-in's future is dropped before it finishes.
        let s = Session::create_in(&root.0).unwrap();
        let dir = s.dir().to_owned();
        fill(&dir);
        let running = async move {
            let _s = s;
            std::future::pending::<()>().await;
        };
        drop(running);
        assert!(!dir.exists());
        // Shared by an email's sign-ins: only the last one lets go of it.
        let shared = std::sync::Arc::new(Session::create_in(&root.0).unwrap());
        let dir = shared.dir().to_owned();
        let step = shared.clone();
        drop(step);
        assert!(dir.exists(), "between an email's sign-ins the profile stays");
        drop(shared);
        assert!(!dir.exists());
        assert_eq!(std::fs::read_dir(&root.0).unwrap().count(), 0);
    }

    #[test]
    fn a_profile_is_overwritten_before_it_is_deleted() {
        let root = TempDir::new();
        let cookies = fill(&root.0);
        let len = std::fs::metadata(&cookies).unwrap().len();
        scrub(&root.0);
        let left = std::fs::read(&cookies).unwrap();
        assert_eq!(left.len() as u64, len);
        assert!(left.iter().all(|b| *b == 0));
        remove_profile(&root.0).unwrap();
        assert!(!root.0.exists());
        remove_profile(&root.0).unwrap();
    }

    #[test]
    fn cleanup_removes_kept_and_left_profiles_but_never_a_running_one() {
        let data = TempDir::new();
        let (legacy, sessions) = (data.0.join("signin-profiles"), data.0.join("signin-sessions"));
        for name in ["0011223344556677", "8899aabbccddeeff"] {
            fill(&legacy.join(name));
        }
        fill(&sessions.join("left-by-a-stopped-slopbar"));
        let running = Session::create_in(&sessions).unwrap();
        fill(running.dir());
        assert_eq!(stale_in(Some(&legacy), Some(&sessions)).len(), 3);
        assert_eq!(cleanup_in(Some(&legacy), Some(&sessions)), Ok(3));
        assert!(!legacy.exists(), "the per-email profiles are gone with their folder");
        assert!(running.dir().exists(), "a sign-in still running keeps its profile");
        assert!(stale_in(Some(&legacy), Some(&sessions)).is_empty());
        assert_eq!(cleanup_in(Some(&legacy), Some(&sessions)), Ok(0), "running again finds nothing");
    }

    #[test]
    fn a_folder_without_a_browser_has_none_to_close() {
        let root = TempDir::new();
        assert!(!browser_open(&root.0));
        close_browser(&root.0);
    }

    #[test]
    fn only_plain_addresses_are_prefilled() {
        assert_eq!(prefill_email(Some("first.last+tag@example.invalid")), Some("first.last+tag@example.invalid"));
        for bad in ["a@b", "a&calc@example.invalid", "a%PATH%@example.invalid", "\"a\"@example.invalid", "a b@example.invalid",
            "-x@example.invalid", "a@-example.invalid", "a@example.invalid^", "no-at-sign", "@example.invalid"] {
            assert_eq!(prefill_email(Some(bad)), None, "{bad}");
        }
        assert_eq!(prefill_email(None), None);
    }

    #[test]
    fn only_provider_pages_over_https_are_opened() {
        for ok in ["https://claude.ai/oauth/authorize?x=1", "https://claude.com/cai/oauth/authorize", "https://platform.claude.com/oauth/authorize",
            "https://auth.openai.com/oauth/authorize?redirect_uri=http%3A%2F%2F127.0.0.1%3A1455"] {
            assert!(allowed_url(ok), "{ok}");
        }
        for bad in ["http://claude.ai/oauth", "https://claude.ai.example.invalid/", "https://evilclaude.ai/", "file:///C:/x", "https://u:p@claude.ai/",
            "https://claude.ai:8443/", "--user-data-dir=C:\\x", "javascript:alert(1)"] {
            assert!(!allowed_url(bad), "{bad}");
        }
    }

    #[test]
    fn browser_arguments_keep_the_url_last() {
        let dir = PathBuf::from(r"C:\data\signin-sessions\0011223344556677");
        let args = browser_args(Kind::Edge, &Target::Profile(dir), "https://claude.ai/oauth");
        assert_eq!(args.first().unwrap(), r"--user-data-dir=C:\data\signin-sessions\0011223344556677");
        assert_eq!(args.last().unwrap(), "https://claude.ai/oauth");
        assert!(browser_args(Kind::Edge, &Target::Private, "https://claude.ai/").contains(&"--inprivate".into()));
        assert!(browser_args(Kind::Chrome, &Target::Private, "https://claude.ai/").contains(&"--incognito".into()));
    }

    #[test]
    fn the_target_survives_the_environment() {
        let dir = PathBuf::from(r"C:\Users\Some One\AppData\Local\dev.thelio.slopbar\signin-sessions\0011223344556677");
        for t in [Target::Private, Target::Profile(dir)] {
            assert_eq!(Target::decode(&t.encode()), Some(t));
        }
        assert_eq!(Target::decode("C:\\elsewhere".as_ref()), None);
        assert_eq!(Target::choose(Window::System, None), None);
        assert_eq!(Target::choose(Window::Private, None), Some(Target::Private));
    }

    #[test]
    fn a_profile_outside_the_root_or_gone_is_refused() {
        let outside = Target::Profile(std::env::temp_dir().join("not-a-signin-profile"));
        assert_eq!(open("https://claude.ai/oauth", &outside), Err("signin_profile_storage".into()));
        if let Some(root) = sessions_root() {
            let deleted = Target::Profile(root.join("a-session-that-ended"));
            assert_eq!(open("https://claude.ai/oauth", &deleted), Err("signin_profile_storage".into()), "never recreated, so never reused");
        }
        assert_eq!(open("https://example.invalid/", &Target::Private), Err("signin_url_rejected".into()));
    }

    /// The real sign-in pages in fresh profiles, for `scripts/test-signin-window.ps1`, which
    /// captures each step on screen; nothing is ever typed or approved. Claude's page opens
    /// through `SLOPBAR_EXE` as its `BROWSER`, exactly as in the app; Codex's through its app
    /// server. Each step writes `<step>.ready` in `SLOPBAR_VISUAL_DIR` and waits for `<step>.go`.
    #[cfg(windows)]
    #[test]
    #[ignore = "opens browser windows; run by scripts/test-signin-window.ps1"]
    fn visual_sign_in_windows() {
        use std::os::windows::process::CommandExt;
        use std::process::{Command, Stdio};
        const CREATE_NEW_CONSOLE: u32 = 0x10;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let sync = PathBuf::from(std::env::var_os("SLOPBAR_VISUAL_DIR").expect("SLOPBAR_VISUAL_DIR"));
        let exe = PathBuf::from(std::env::var_os("SLOPBAR_EXE").expect("SLOPBAR_EXE"));
        assert!(std::env::var_os("SLOPBAR_DATA_DIR").is_some(), "keep the sessions out of the installed app's folder");
        let step = |name: &str, report: String| {
            std::fs::write(sync.join(format!("{name}.ready")), &report).unwrap();
            println!("{name}: {report}");
            let go = sync.join(format!("{name}.go"));
            let until = std::time::Instant::now() + Duration::from_secs(180);
            while !go.exists() && std::time::Instant::now() < until {
                std::thread::sleep(Duration::from_millis(200));
            }
        };
        let browser = |dir: &Path| {
            let until = std::time::Instant::now() + Duration::from_secs(45);
            loop {
                if let Some(pid) = browser_pid(dir) {
                    std::thread::sleep(Duration::from_secs(4));
                    return Some(pid);
                }
                if std::time::Instant::now() > until {
                    return None;
                }
                std::thread::sleep(Duration::from_millis(250));
            }
        };
        let claude = |session: &Session, email: &str, home: &Path| {
            Command::new("claude").args(["--setting-sources", "user", "auth", "login", "--email", email])
                .env("CLAUDE_CONFIG_DIR", home).envs(claude_browser_env_for(&exe, &session.target()))
                .creation_flags(CREATE_NEW_CONSOLE).spawn().expect("claude")
        };

        let first = Session::create().expect("session");
        let homes = [TempDir::new(), TempDir::new(), TempDir::new()];
        let mut claude_one = claude(&first, "visual.one@example.invalid", &homes[0].0);
        let pid = browser(first.dir());
        step("1-claude", format!("browser={pid:?} profile={}", first.dir().display()));

        std::fs::write(homes[1].0.join("config.toml"), "cli_auth_credentials_store = \"file\"\n").unwrap();
        let mut codex = Command::new("codex").arg("app-server").env("CODEX_HOME", &homes[1].0)
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).creation_flags(CREATE_NO_WINDOW).spawn().expect("codex");
        let (stdin, stdout) = (codex.stdin.take().unwrap(), codex.stdout.take().unwrap());
        let target = first.target();
        let started = crate::codex_signin::start(stdin, stdout, |url| open(url, &target));
        std::thread::sleep(Duration::from_secs(6));
        let again = browser_pid(first.dir());
        step("2-codex", format!("started={} browser={again:?} same_browser={}", started.is_ok(), again.is_some() && again == pid));

        let _ = claude_one.kill();
        drop(started);
        let _ = codex.kill();
        let dir = first.dir().to_owned();
        drop(first);
        step("3-closed", format!("browser_open={} profile_exists={}", browser_open(&dir), dir.exists()));

        let second = Session::create().expect("session");
        let mut claude_two = claude(&second, "visual.two@example.invalid", &homes[2].0);
        let next = browser(second.dir());
        step("4-next", format!("browser={next:?} new_profile={} new_browser={}", second.dir() != dir, next.is_some() && next != pid));

        let _ = claude_two.kill();
        let dir = second.dir().to_owned();
        drop(second);
        step("5-closed", format!("browser_open={} profile_exists={}", browser_open(&dir), dir.exists()));
    }

    /// [`claude_browser_env`] with another launcher than this test binary.
    #[cfg(windows)]
    fn claude_browser_env_for(exe: &Path, target: &Target) -> [(OsString, OsString); 2] {
        [("BROWSER".into(), short_path(exe).unwrap_or(exe.to_owned()).into_os_string()), (TARGET_ENV.into(), target.encode())]
    }
}
