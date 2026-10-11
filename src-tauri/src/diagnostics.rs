//! Explicitly allowlisted diagnostics. Never serialize a Slot, Snapshot, error message,
//! request, environment, filesystem path, or provider response into an export.
use crate::{accounts::Provider, model::Snapshot, private_store, settings::Settings};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Mutex;

const MAX_EVENTS: usize = 10_000;
const MAX_ACCOUNTS: usize = 1_024;
const MAX_BYTES: u64 = 16 * 1024 * 1024;
const DAYS: i64 = 7;

#[derive(Clone, Copy, Serialize, Deserialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Code {
    Ok,
    Setup,
    Reconnect,
    Storage,
    Network,
    WrongAccount,
    SharedLogin,
    LoginFailed,
    Busy,
    Uncertain,
    RateLimited,
    TokenInvalid,
    TokenExpired,
    BadResponse,
    Http(u16),
    Other,
}
impl Code {
    pub fn from_error(error: Option<&str>) -> Self {
        match error {
            None => Self::Ok,
            Some("monitor_setup") => Self::Setup,
            Some("monitor_reconnect") => Self::Reconnect,
            Some("monitor_storage" | "cache_storage") => Self::Storage,
            Some("monitor_network") => Self::Network,
            Some("monitor_wrong_account") => Self::WrongAccount,
            Some("monitor_shared_login") => Self::SharedLogin,
            Some("monitor_login_failed") => Self::LoginFailed,
            Some("monitor_busy") => Self::Busy,
            Some("monitor_uncertain") => Self::Uncertain,
            Some("rate_limited") => Self::RateLimited,
            Some("token_invalid") => Self::TokenInvalid,
            Some("token_expired") => Self::TokenExpired,
            Some("bad_response") => Self::BadResponse,
            Some(e)
                if e.strip_prefix("rate_limited:")
                    .is_some_and(|n| n.parse::<i64>().is_ok()) =>
            {
                Self::RateLimited
            }
            Some(e) => e
                .strip_prefix("http_")
                .and_then(|n| n.parse::<u16>().ok())
                .filter(|n| (100..=599).contains(n))
                .map(Self::Http)
                .unwrap_or(Self::Other),
        }
    }
}
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    ConnectStarted,
    ConnectFinished,
    Disconnect,
    Remove,
    SwitchManual,
    SwitchAuto,
    Cleanup,
    CacheSave,
    MonitorSave,
    TokenRead,
    Revoke,
    Export,
}
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Surface {
    Settings,
    Panel,
    Dock,
}
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrontendKind {
    Error,
    TypeError,
    ReferenceError,
    SyntaxError,
    RangeError,
    Rejection,
    Other,
}
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    Monitoring,
    Cli,
    ActiveCliFallback,
}
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Reuse,
    Grace,
    Backoff,
    RetryExhausted,
    Reconnect,
    StorageFailure,
    UsageRejected,
    LateRejectionIgnored,
}
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefreshReason {
    AccessExpiry,
    UsageRejection,
    ExpiryHint,
}
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefreshOutcome {
    Started,
    Success,
    Revoked,
    RotatedUnreadable,
    Transient,
    RateLimited,
    Ambiguous,
}
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoginStage {
    CliDiscovery,
    Directory,
    Prepare,
    ProcessStart,
    ProcessExit,
    Timeout,
    Credentials,
    Profile,
    Handoff,
    FolderCleanup,
    Cancelled,
}
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RevokeReason {
    Disconnect,
    Replacement,
    UninstalledLogin,
}
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RevokeOutcome {
    Confirmed,
    Unconfirmed,
    Protected,
    UnknownOwnership,
    NoToken,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    Http {
        endpoint: Endpoint,
        status: Option<u16>,
        network: Option<Network>,
        elapsed_ms: u64,
    },
    Startup {
        previous_clean_shutdown: Option<bool>,
        history_unreadable: bool,
    },
    Shutdown,
    PollStarted {
        gap_seconds: Option<i64>,
    },
    PollFinished {
        elapsed_ms: u64,
        accounts: usize,
        native_sources: usize,
        wsl_sources: usize,
    },
    Action {
        action: Action,
        code: Code,
        elapsed_ms: u64,
    },
    Refresh {
        outcome: RefreshOutcome,
        reason: RefreshReason,
        elapsed_ms: u64,
    },
    Token {
        decision: Decision,
        access_expires_at: Option<DateTime<Utc>>,
        retry_at: Option<DateTime<Utc>>,
        uncertain_attempts: u8,
        access_rejected: bool,
        usable: bool,
    },
    Usage {
        origin: Origin,
        code: Code,
        elapsed_ms: u64,
    },
    Frontend {
        surface: Surface,
        kind: FrontendKind,
        line: Option<u32>,
        column: Option<u32>,
    },
    Login {
        stage: LoginStage,
        ok: bool,
        wsl: bool,
        exit_code: Option<i32>,
        elapsed_ms: u64,
    },
    Revoke {
        reason: RevokeReason,
        outcome: RevokeOutcome,
        elapsed_ms: u64,
    },
    Sample {
        active: bool,
        monitoring: bool,
        stale: bool,
        last_confirmed_at: Option<DateTime<Utc>>,
        code: Code,
        monitor_code: Code,
        windows: Vec<Window>,
    },
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Window {
    kind: WindowKind,
    used_percent: f64,
    resets_at: Option<DateTime<Utc>>,
}
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Endpoint {
    Token,
    Revoke,
}
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Network {
    Connect,
    Timeout,
    Transport,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum WindowKind {
    Session,
    Weekly,
    WeeklyOpus,
    WeeklySonnet,
    Window,
    Other,
}
fn sample(a: &crate::model::AccountUsage) -> Event {
    Event::Sample {
        active: a.active,
        monitoring: a.monitor_connected,
        stale: a.stale,
        last_confirmed_at: a.fetched_at,
        code: Code::from_error(a.error.as_deref()),
        monitor_code: Code::from_error(a.monitor_error.as_deref()),
        windows: a
            .windows
            .iter()
            .take(8)
            .filter(|w| w.used_percent.is_finite())
            .map(|w| Window {
                kind: match w.kind.as_str() {
                    "session" => WindowKind::Session,
                    "weekly" => WindowKind::Weekly,
                    "weekly_opus" => WindowKind::WeeklyOpus,
                    "weekly_sonnet" => WindowKind::WeeklySonnet,
                    "window" => WindowKind::Window,
                    _ => WindowKind::Other,
                },
                used_percent: w.used_percent.clamp(0.0, 100.0),
                resets_at: w.resets_at,
            })
            .collect(),
    }
}
#[derive(Clone, Serialize, Deserialize)]
struct Record {
    at: DateTime<Utc>,
    run: u64,
    provider: Option<Provider>,
    account: Option<u32>,
    event: Event,
}
#[derive(Default, Serialize, Deserialize)]
struct History {
    version: u8,
    run: u64,
    next_account: u32,
    // Only in the encrypted local journal. The report NEVER includes this mapping.
    accounts: HashMap<String, u32>,
    events: VecDeque<Record>,
    dropped: u64,
    clean_shutdown: bool,
}
#[derive(Default)]
struct State {
    history: History,
    started: bool,
    history_unreadable: bool,
    storage_errors: u64,
    last_poll: Option<DateTime<Utc>>,
}
pub struct Recorder {
    path: PathBuf,
    state: Mutex<State>,
}
pub struct Context {
    pub recorder: std::sync::Arc<Recorder>,
    pub provider: Provider,
    pub id: String,
}
impl Context {
    pub fn login(
        &self,
        stage: LoginStage,
        ok: bool,
        wsl: bool,
        exit_code: Option<i32>,
        elapsed_ms: u64,
    ) {
        self.recorder.record(
            Some(self.provider),
            Some(&self.id),
            Event::Login {
                stage,
                ok,
                wsl,
                exit_code,
                elapsed_ms,
            },
        );
    }
}
impl Recorder {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            state: Mutex::default(),
        }
    }
    // Start only in Tauri setup, AFTER single-instance handling. A second launch
    // must not load/save a competing journal or falsely mark the app as stopped.
    pub fn start(&self) {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if s.started {
            return;
        }
        if self.path.exists() {
            let loaded = std::fs::metadata(&self.path)
                .ok()
                .filter(|m| m.len() <= MAX_BYTES)
                .and_then(|_| private_store::load_secret(&self.path).ok())
                .and_then(|b| serde_json::from_slice::<History>(&b).ok())
                .filter(|h| {
                    h.version == 1
                        && h.accounts.len() <= MAX_ACCOUNTS
                        && h.events.len() <= MAX_EVENTS
                });
            if let Some(h) = loaded {
                s.history = h;
            } else {
                s.history_unreadable = true;
            }
        }
        let clean = (s.history.version == 1).then_some(s.history.clean_shutdown);
        s.history.version = 1;
        s.history.run = s.history.run.saturating_add(1);
        s.history.clean_shutdown = false;
        s.started = true;
        let unreadable = s.history_unreadable;
        Self::push(
            &mut s,
            None,
            None,
            Event::Startup {
                previous_clean_shutdown: clean,
                history_unreadable: unreadable,
            },
        );
        self.persist(&mut s);
    }
    fn account(s: &mut State, provider: Option<Provider>, id: Option<&str>) -> Option<u32> {
        let (provider, id) = (provider?, id?);
        if id.len() > 512 {
            return None;
        }
        let key = format!("{}:{id}", provider.name());
        if let Some(n) = s.history.accounts.get(&key) {
            return Some(*n);
        }
        if s.history.accounts.len() >= MAX_ACCOUNTS {
            return None;
        }
        s.history.next_account = s.history.next_account.saturating_add(1);
        let n = s.history.next_account;
        s.history.accounts.insert(key, n);
        Some(n)
    }
    fn push(s: &mut State, provider: Option<Provider>, id: Option<&str>, event: Event) {
        let now = Utc::now();
        let account = Self::account(s, provider, id);
        s.history.events.push_back(Record {
            at: now,
            run: s.history.run,
            provider,
            account,
            event,
        });
        while s.history.events.len() > MAX_EVENTS
            || s.history
                .events
                .front()
                .is_some_and(|r| r.at < now - chrono::Duration::days(DAYS))
        {
            s.history.events.pop_front();
            s.history.dropped = s.history.dropped.saturating_add(1);
        }
    }
    fn persist(&self, s: &mut State) {
        let ok = serde_json::to_vec(&s.history)
            .ok()
            .filter(|b| b.len() as u64 <= MAX_BYTES)
            .is_some_and(|b| private_store::save_secret(&self.path, &b).is_ok());
        if !ok {
            s.storage_errors = s.storage_errors.saturating_add(1);
        }
    }
    pub fn record(&self, provider: Option<Provider>, id: Option<&str>, event: Event) {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !s.started {
            return;
        }
        if matches!(event, Event::Shutdown) {
            s.history.clean_shutdown = true;
        }
        // Batch routine observations into the poll's durable completion. Preserve
        // lifecycle/auth transitions immediately, including BEFORE sending a refresh.
        let flush = !matches!(
            event,
            Event::Token { .. } | Event::Usage { .. } | Event::Frontend { .. } | Event::Http { .. }
        );
        Self::push(&mut s, provider, id, event);
        // Best effort only: diagnostic disk failure NEVER changes authentication.
        if flush {
            self.persist(&mut s);
        }
    }
    pub fn poll_started(&self) {
        let gap = {
            let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            s.last_poll
                .replace(Utc::now())
                .map(|t| (Utc::now() - t).num_seconds())
        };
        self.record(None, None, Event::PollStarted { gap_seconds: gap });
    }
    pub fn samples(&self, snapshot: &Snapshot) {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !s.started {
            return;
        }
        for a in &snapshot.accounts {
            Self::push(&mut s, Some(a.provider), Some(&a.id), sample(a));
        }
    }
    pub fn report(
        &self,
        snapshot: &Snapshot,
        settings: &Settings,
    ) -> Result<Vec<u8>, &'static str> {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !s.started {
            return Err("diagnostic_storage");
        }
        let current: Vec<Record> = snapshot
            .accounts
            .iter()
            .map(|a| Record {
                at: Utc::now(),
                run: s.history.run,
                provider: Some(a.provider),
                account: Self::account(&mut s, Some(a.provider), Some(&a.id)),
                event: sample(a),
            })
            .collect();
        #[derive(Serialize)]
        struct Report<'a> {
            schema_version: u8,
            exported_at: DateTime<Utc>,
            build: Build,
            retention_days: i64,
            event_limit: usize,
            dropped_events: u64,
            history_unreadable: bool,
            storage_errors: u64,
            refresh_minutes: u32,
            claude_enabled: bool,
            codex_enabled: bool,
            current: Vec<Record>,
            events: &'a VecDeque<Record>,
        }
        let report = Report {
            schema_version: 1,
            exported_at: Utc::now(),
            build: Build::current(),
            retention_days: DAYS,
            event_limit: MAX_EVENTS,
            dropped_events: s.history.dropped,
            history_unreadable: s.history_unreadable,
            storage_errors: s.storage_errors,
            refresh_minutes: settings.refresh_minutes,
            claude_enabled: settings.providers.claude,
            codex_enabled: settings.providers.codex,
            current,
            events: &s.history.events,
        };
        serde_json::to_vec_pretty(&report).map_err(|_| "diagnostic_storage")
    }
    pub fn export(
        &self,
        snapshot: &Snapshot,
        settings: &Settings,
    ) -> Result<PathBuf, &'static str> {
        self.record(
            None,
            None,
            Event::Action {
                action: Action::Export,
                code: Code::Ok,
                elapsed_ms: 0,
            },
        );
        let bytes = self.report(snapshot, settings)?;
        let dir = self
            .path
            .parent()
            .ok_or("diagnostic_storage")?
            .join("exports");
        let path = dir.join(format!(
            "SlopBar-Diagnostics-{}.json",
            Utc::now().format("%Y%m%d-%H%M%S-%f")
        ));
        private_store::write(&path, &bytes).map_err(|_| "diagnostic_storage")?;
        Ok(path)
    }
}
#[derive(Serialize)]
struct Build {
    version: &'static str,
    commit: Option<&'static str>,
    source_tree: Option<&'static str>,
    dirty: bool,
    os: &'static str,
    arch: &'static str,
    upstream_updates: bool,
    tauri_dev: Option<bool>,
}
impl Build {
    fn current() -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION"),
            commit: option_env!("SLOPBAR_BUILD_COMMIT"),
            source_tree: option_env!("SLOPBAR_BUILD_TREE"),
            dirty: option_env!("SLOPBAR_BUILD_DIRTY") != Some("false"),
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
            upstream_updates: cfg!(feature = "upstream-updates"),
            tauri_dev: option_env!("SLOPBAR_TAURI_DEV").and_then(|s| s.parse().ok()),
        }
    }
}
pub fn write_build_info(path: &std::path::Path) -> std::io::Result<()> {
    let bytes = serde_json::to_vec_pretty(&Build::current()).map_err(std::io::Error::other)?;
    std::fs::write(path, bytes)
}
pub fn millis(start: std::time::Instant) -> u64 {
    start.elapsed().as_millis().min(u64::MAX as u128) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    #[test]
    fn export_allowlist_excludes_identities_credentials_paths_and_raw_errors() {
        let dir = TempDir::new();
        let r = Recorder::new(dir.0.join("history"));
        r.start();
        let canary = "PRIVATE-canary-access-refresh-email-org-path-url";
        let a = crate::model::AccountUsage {
            id: canary.into(),
            provider: Provider::Claude,
            email: Some(canary.into()),
            alias: Some(canary.into()),
            plan: Some(canary.into()),
            org: Some(canary.into()),
            active: false,
            sources: vec![canary.into()],
            windows: vec![crate::model::UsageWindow {
                kind: canary.into(),
                used_percent: 87.0,
                window_seconds: None,
                resets_at: None,
            }],
            credits: Some(canary.into()),
            resets: vec![],
            error: Some(canary.into()),
            monitor_error: Some(canary.into()),
            stale: true,
            fetched_at: None,
            monitor_connected: true,
        };
        let snapshot = Snapshot {
            accounts: vec![a],
            updated_at: None,
        };
        let mut settings = Settings::default();
        settings.aliases.insert(canary.into(), canary.into());
        r.samples(&snapshot);
        #[cfg(windows)]
        assert!(!std::fs::read(dir.0.join("history"))
            .unwrap()
            .windows(canary.len())
            .any(|b| b == canary.as_bytes()));
        let path = r.export(&snapshot, &settings).unwrap();
        let report = std::fs::read_to_string(path).unwrap();
        assert!(!report.contains(canary));
        assert!(!report.contains(&dir.0.to_string_lossy().to_string()));
        assert!(report.contains("87.0"));
        assert!(report.contains("\"account\": 1"));
        let json: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(json["current"][0]["event"]["code"], "other");
        assert_eq!(json["current"][0]["event"]["windows"][0]["kind"], "other");
    }
    #[test]
    fn pseudonyms_history_and_unclean_exit_survive_restart() {
        let dir = TempDir::new();
        let path = dir.0.join("history");
        let r = Recorder::new(path.clone());
        r.start();
        r.record(
            Some(Provider::Claude),
            Some("private-id"),
            Event::Action {
                action: Action::ConnectStarted,
                code: Code::Ok,
                elapsed_ms: 0,
            },
        );
        drop(r);
        let r = Recorder::new(path);
        r.start();
        r.record(
            Some(Provider::Claude),
            Some("private-id"),
            Event::Action {
                action: Action::ConnectFinished,
                code: Code::Ok,
                elapsed_ms: 1,
            },
        );
        let report: serde_json::Value = serde_json::from_slice(
            &r.report(&Snapshot::default(), &Settings::default())
                .unwrap(),
        )
        .unwrap();
        assert_eq!(report["events"][1]["account"], 1);
        assert_eq!(report["events"][3]["account"], 1);
        assert_eq!(
            report["events"][2]["event"]["previous_clean_shutdown"],
            false
        );
        assert!(!serde_json::to_string(&report)
            .unwrap()
            .contains("private-id"));
    }
    #[test]
    fn rotation_corruption_and_disk_failure_are_visible_and_nonfatal() {
        let dir = TempDir::new();
        let path = dir.0.join("history");
        std::fs::write(&path, b"broken").unwrap();
        let r = Recorder::new(path.clone());
        r.start();
        {
            let mut s = r.state.lock().unwrap();
            let old = Record {
                at: Utc::now() - chrono::Duration::days(8),
                run: 1,
                provider: None,
                account: None,
                event: Event::Shutdown,
            };
            s.history.events = VecDeque::from(vec![old; MAX_EVENTS + 1]);
        }
        r.record(None, None, Event::Shutdown);
        assert_eq!(r.state.lock().unwrap().history.events.len(), 1);
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        r.record(
            None,
            None,
            Event::PollStarted {
                gap_seconds: Some(900),
            },
        );
        let report: serde_json::Value = serde_json::from_slice(
            &r.report(&Snapshot::default(), &Settings::default())
                .unwrap(),
        )
        .unwrap();
        assert_eq!(report["history_unreadable"], true);
        assert!(report["storage_errors"].as_u64().unwrap() > 0);
        assert!(report["dropped_events"].as_u64().unwrap() > MAX_EVENTS as u64);
    }
    #[test]
    fn error_codes_never_passthrough_messages() {
        assert_eq!(
            Code::from_error(Some("rate_limited:900")),
            Code::RateLimited
        );
        assert_eq!(Code::from_error(Some("http_503")), Code::Http(503));
        for e in [
            "http_503?token=secret",
            "http_999",
            "monitor_network:secret",
            "https://private/path",
            "rate_limited:secret",
        ] {
            assert_eq!(Code::from_error(Some(e)), Code::Other);
        }
    }
}
