//! Which accounts are in use on the user's other machines.
//!
//! Neither Anthropic nor OpenAI tells a token where else its account is signed in, so SlopBar
//! learns it two ways:
//!
//! - **Presence files** (opt-in). Every SlopBar writes `<folder>/SlopBar/presence/<machine id>.json`
//!   into a folder the user already syncs (OneDrive, Dropbox, iCloud Drive), listing which accounts
//!   it has signed in. Accounts appear there only as a unsalted SHA-256 prefix of
//!   `provider|slot id`, which every machine computes the same way; no emails, tokens or
//!   organization names are written.
//! - **Inference**, for monitored accounts nobody claims: usage that keeps rising while the
//!   account is signed in nowhere SlopBar can see means it is used outside this PC (another
//!   machine, the web or the desktop app).
//!
//! `AccountUsage::active` and `sources` keep meaning *this machine*: switching, the remove guard
//! and the balancer depend on that. Other machines are reported in `remote` only.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::accounts::Provider;
use crate::model::{AccountUsage, RemoteSeat};

/// Presence files live here, under the folder the user picked.
const SUBDIR: [&str; 2] = ["SlopBar", "presence"];
/// A machine counts as live for at least this long after it last wrote.
const MIN_LIVE: i64 = 15 * 60;
/// Machines that stopped writing are shown dimmed ("seen 3h ago") this long.
const SHOW_FOR: i64 = 24 * 3600;
/// Files older than this are ignored altogether.
const IGNORE_AFTER: i64 = 7 * 24 * 3600;
/// A clock this far ahead of ours is not trusted; the file's modification time is used instead.
const SKEW: i64 = 5 * 60;
/// Machine names longer than this are cut (they come from other machines).
const NAME_MAX: usize = 63;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Seat {
    pub p: Provider,
    /// `hash(provider, slot id)`.
    pub h: String,
    #[serde(rename = "where", default)]
    pub where_: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Presence {
    pub v: u32,
    pub machine_id: String,
    pub machine: String,
    pub written_at: DateTime<Utc>,
    pub interval_s: u64,
    #[serde(default)]
    pub seats: Vec<Seat>,
}

/// How another machine's account appears on every machine: the first 32 hex digits of
/// SHA-256(`provider|slot id`). Stable across versions and platforms (unlike `DefaultHasher`).
pub fn hash(provider: Provider, id: &str) -> String {
    let digest = Sha256::digest(format!("{}|{id}", provider.key()).as_bytes());
    digest.iter().take(16).map(|b| format!("{b:02x}")).collect()
}

pub fn dir(folder: &Path) -> PathBuf {
    SUBDIR.iter().fold(folder.to_path_buf(), |p, s| p.join(s))
}

/// This installation's id: random, kept in the app's data folder, never derived from the user.
pub fn machine_id() -> Option<String> {
    let path = crate::settings::app_dir()?.join("machine-id");
    if let Ok(id) = std::fs::read_to_string(&path) {
        let id = id.trim();
        if uuid::Uuid::parse_str(id).is_ok() {
            return Some(id.to_string());
        }
    }
    let id = uuid::Uuid::new_v4().to_string();
    crate::private_store::write(&path, id.as_bytes()).ok()?;
    Some(id)
}

/// This machine's id and name, read once.
pub fn identity() -> Option<&'static (String, String)> {
    static IDENTITY: std::sync::OnceLock<Option<(String, String)>> = std::sync::OnceLock::new();
    IDENTITY.get_or_init(|| Some((machine_id()?, machine_name()))).as_ref()
}

/// Says this machine has nothing signed in any more: on quit, or when sharing moves elsewhere.
pub fn withdraw(folder: &Path, interval_s: u64) {
    if let Some((id, name)) = identity() {
        let _ = write(folder, &own(id, name, interval_s, &[], Utc::now()));
    }
}

/// The computer's name, as other machines will show it.
pub fn machine_name() -> String {
    let name = std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_default();
    clean_name(&name).unwrap_or_else(|| "?".into())
}

fn clean_name(name: &str) -> Option<String> {
    let name: String = name.trim().chars().filter(|c| !c.is_control()).take(NAME_MAX).collect();
    (!name.is_empty()).then_some(name)
}

/// A folder a sync client keeps the same on every machine.
#[derive(Debug, Clone, Serialize)]
pub struct SyncFolder {
    pub name: &'static str,
    pub path: String,
}

/// The sync folders on this machine that SlopBar can offer.
pub fn sync_folders() -> Vec<SyncFolder> {
    let home = dirs::home_dir();
    let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
    // Dropbox records where its folder is; ~/Dropbox is only the default.
    let dropbox = dirs::data_local_dir()
        .and_then(|d| std::fs::read_to_string(d.join("Dropbox").join("info.json")).ok())
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| ["personal", "business"].iter().find_map(|k| v[k]["path"].as_str().map(PathBuf::from)));
    let candidates = [
        ("OneDrive", env("OneDriveConsumer").or_else(|| env("OneDrive"))),
        ("OneDrive", env("OneDriveCommercial")),
        ("Dropbox", dropbox.or_else(|| home.as_ref().map(|h| h.join("Dropbox")))),
        ("iCloud Drive", home.as_ref().map(|h| h.join("iCloudDrive"))),
    ];
    let mut out: Vec<SyncFolder> = vec![];
    for (name, path) in candidates {
        let Some(path) = path.filter(|p| p.is_dir()) else { continue };
        let path = path.to_string_lossy().into_owned();
        if !out.iter().any(|f| f.path.eq_ignore_ascii_case(&path)) {
            out.push(SyncFolder { name, path });
        }
    }
    out
}

/// What this machine says about itself. Logins SlopBar could only identify by token
/// (`token:` slot ids) are left out: their id would change with the token.
pub fn own(machine_id: &str, machine: &str, interval_s: u64, accounts: &[AccountUsage], now: DateTime<Utc>) -> Presence {
    let mut seats: Vec<Seat> = accounts
        .iter()
        .filter(|a| a.active && !a.id.starts_with("token:"))
        .map(|a| Seat { p: a.provider, h: hash(a.provider, &a.id), where_: a.sources.clone() })
        .collect();
    seats.sort_by(|a, b| (a.p.key(), &a.h).cmp(&(b.p.key(), &b.h)));
    Presence { v: 1, machine_id: machine_id.into(), machine: machine.into(), written_at: now, interval_s, seats }
}

/// Writes this machine's file atomically (sync clients never upload half a file).
pub fn write(folder: &Path, presence: &Presence) -> std::io::Result<()> {
    if !folder.is_dir() {
        return Err(std::io::Error::other("presence folder missing"));
    }
    let safe: String = presence.machine_id.chars().filter(|c| c.is_ascii_hexdigit() || *c == '-').collect();
    let bytes = serde_json::to_vec_pretty(presence)?;
    crate::private_store::write(&dir(folder).join(format!("{safe}.json")), &bytes)
}

/// Another machine, as of its newest file.
#[derive(Debug, Clone)]
pub struct Peer {
    pub machine: String,
    pub seen_at: DateTime<Utc>,
    pub live: bool,
    pub seats: Vec<Seat>,
}

/// Every other machine that wrote within the last week. Sync clients leave conflict copies
/// ("<id> (1).json", "<id>-LAPTOP.json"), so files are grouped by the machine id inside them
/// and the newest wins.
pub fn read(folder: &Path, own_id: &str, now: DateTime<Utc>) -> Vec<Peer> {
    let Ok(entries) = std::fs::read_dir(dir(folder)) else { return vec![] };
    let files = entries.flatten().filter_map(|e| {
        let path = e.path();
        if path.extension().and_then(|x| x.to_str()) != Some("json") {
            return None;
        }
        let modified = e.metadata().ok()?.modified().ok().map(DateTime::<Utc>::from);
        // Presence files are small; anything large is not one.
        if e.metadata().ok()?.len() > 256 * 1024 {
            return None;
        }
        let presence: Presence = serde_json::from_slice(&std::fs::read(&path).ok()?).ok()?;
        Some((presence, modified))
    });
    peers(files, own_id, now)
}

pub fn peers(files: impl IntoIterator<Item = (Presence, Option<DateTime<Utc>>)>, own_id: &str, now: DateTime<Utc>) -> Vec<Peer> {
    let mut newest: HashMap<String, (Presence, DateTime<Utc>)> = HashMap::new();
    for (presence, modified) in files {
        if presence.v != 1 || presence.machine_id.is_empty() || presence.machine_id == own_id {
            continue;
        }
        // A clock running ahead would keep a dead machine "live" forever: trust the file's own
        // modification time then, and never a moment after now.
        let seen = if presence.written_at > now + Duration::seconds(SKEW) {
            modified.unwrap_or(now - Duration::seconds(IGNORE_AFTER + 1)).min(now)
        } else {
            presence.written_at.min(now)
        };
        match newest.get(&presence.machine_id) {
            Some((_, at)) if *at >= seen => {}
            _ => {
                newest.insert(presence.machine_id.clone(), (presence, seen));
            }
        }
    }
    let mut out: Vec<Peer> = newest
        .into_values()
        .filter_map(|(p, seen)| {
            let age = (now - seen).num_seconds();
            if age > IGNORE_AFTER {
                return None;
            }
            let live_for = (p.interval_s.min(24 * 3600) as i64 * 3).max(MIN_LIVE);
            Some(Peer {
                machine: clean_name(&p.machine).unwrap_or_else(|| "?".into()),
                seen_at: seen,
                live: age <= live_for,
                seats: p.seats,
            })
        })
        .collect();
    out.sort_by(|a, b| a.machine.cmp(&b.machine));
    out
}

/// Fills each account's `remote` from the other machines' files: live ones, and ones that
/// stopped writing within the last day (dimmed).
pub fn attach(accounts: &mut [AccountUsage], peers: &[Peer], now: DateTime<Utc>) {
    for a in accounts.iter_mut() {
        let h = hash(a.provider, &a.id);
        a.remote = peers
            .iter()
            .filter(|p| p.live || (now - p.seen_at).num_seconds() <= SHOW_FOR)
            .filter_map(|p| {
                let seat = p.seats.iter().find(|s| s.p == a.provider && s.h == h)?;
                Some(RemoteSeat {
                    machine: p.machine.clone(),
                    where_: seat.where_.iter().filter_map(|w| clean_name(w)).take(8).collect(),
                    seen_at: p.seen_at,
                    live: p.live,
                })
            })
            .collect();
    }
}

// ---------------------------------------------------------------------------------------------
// Inference: used outside this PC

/// What the last fresh reading of one account looked like.
#[derive(Debug, Clone, Default)]
pub struct Track {
    fetched_at: Option<DateTime<Utc>>,
    /// (window kind, reset time, percent used)
    windows: Vec<(String, Option<DateTime<Utc>>, f64)>,
    /// Whether the account was unclaimed (not in use here or on a live peer) when read.
    unclaimed: bool,
    last_rise: Option<DateTime<Utc>>,
}

pub type Tracks = HashMap<(Provider, String), Track>;

/// Flags monitored accounts that nobody SlopBar can see is using, yet whose usage rose between
/// two consecutive fresh readings of the same window (same reset time) by at least one point.
/// The flag clears once `hold` passes without another rise.
pub fn infer(tracks: &mut Tracks, accounts: &mut [AccountUsage], hold: Duration, now: DateTime<Utc>) {
    tracks.retain(|(p, id), _| accounts.iter().any(|a| a.provider == *p && a.id == *id));
    for a in accounts.iter_mut() {
        let unclaimed = a.monitor_connected && !a.active && !a.remote.iter().any(|r| r.live);
        let track = tracks.entry((a.provider, a.id.clone())).or_default();
        let fresh = !a.stale && a.error.is_none() && a.monitor_error.is_none() && a.fetched_at.is_some();
        if fresh && a.fetched_at != track.fetched_at {
            let rose = unclaimed
                && track.unclaimed
                && a.windows.iter().any(|w| {
                    track.windows.iter().any(|(kind, resets, before)| {
                        *kind == w.kind && resets.is_some() && *resets == w.resets_at && w.used_percent - before >= 1.0
                    })
                });
            if rose {
                track.last_rise = a.fetched_at;
            }
            track.fetched_at = a.fetched_at;
            track.windows = a.windows.iter().map(|w| (w.kind.clone(), w.resets_at, w.used_percent)).collect();
            track.unclaimed = unclaimed;
        } else if !fresh {
            // A failed reading breaks the chain: the next fresh one has nothing to compare with.
            track.windows.clear();
        } else if !unclaimed {
            track.unclaimed = false;
        }
        if !unclaimed {
            track.last_rise = None;
        }
        a.used_outside = track.last_rise.is_some_and(|t| now - t <= hold);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::UsageWindow;
    use crate::test_support::TempDir;

    fn at(minutes: i64) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-10-21T12:00:00Z").unwrap().with_timezone(&Utc) + Duration::minutes(minutes)
    }

    fn account(id: &str, active: bool) -> AccountUsage {
        AccountUsage {
            id: id.into(),
            provider: Provider::Claude,
            email: Some(format!("{id}@example.invalid")),
            alias: None,
            plan: Some("max".into()),
            org: Some("Synthetic Org".into()),
            active,
            sources: if active { vec!["Windows".into(), "WSL: Ubuntu".into()] } else { vec![] },
            windows: vec![],
            credits: None,
            resets: vec![],
            error: None,
            monitor_error: None,
            stale: false,
            fetched_at: None,
            monitor_connected: true,
            billing: Default::default(),
            remote: vec![],
            used_outside: false,
        }
    }

    fn file(id: &str, machine: &str, written: DateTime<Utc>, seats: Vec<Seat>) -> Presence {
        Presence { v: 1, machine_id: id.into(), machine: machine.into(), written_at: written, interval_s: 300, seats }
    }

    fn seat(id: &str) -> Seat {
        Seat { p: Provider::Claude, h: hash(Provider::Claude, id), where_: vec!["Windows".into()] }
    }

    #[test]
    fn hashing_is_stable_and_provider_specific() {
        // Fixed vector (printf "claude|synthetic-account:synthetic-org" | sha256sum): every machine
        // and every future version must compute the same value.
        assert_eq!(hash(Provider::Claude, "synthetic-account:synthetic-org"), "95ebf9717011b553004c8c1c6a6241f1");
        assert_eq!(hash(Provider::Claude, "a").len(), 32);
        assert_ne!(hash(Provider::Claude, "a"), hash(Provider::Codex, "a"));
    }

    #[test]
    fn conflict_copies_resolve_to_the_newest_and_own_file_is_ignored() {
        let now = at(0);
        let peers = peers(
            [
                (file("peer", "LAPTOP-2", at(-30), vec![seat("old")]), None),
                (file("peer", "LAPTOP-2", at(-2), vec![seat("new")]), None),
                (file("peer", "LAPTOP-2", at(-10), vec![seat("mid")]), None),
                (file("me", "BACHELOR", at(-1), vec![seat("new")]), None),
            ],
            "me",
            now,
        );
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].seats, vec![seat("new")]);
        assert!(peers[0].live);
    }

    #[test]
    fn staleness_windows() {
        let now = at(0);
        let list = peers(
            [
                (file("a", "A", at(-14), vec![]), None),          // within 15 min floor
                (file("b", "B", at(-16), vec![]), None),          // past 3 × 5 min and 15 min
                (Presence { interval_s: 1800, ..file("c", "C", at(-80), vec![]) }, None), // 3 × 30 min
                (file("d", "D", at(-6 * 24 * 60), vec![]), None), // kept, not live
                (file("e", "E", at(-8 * 24 * 60), vec![]), None), // ignored
            ],
            "me",
            now,
        );
        let live: HashMap<_, _> = list.iter().map(|p| (p.machine.as_str(), p.live)).collect();
        assert_eq!(live, HashMap::from([("A", true), ("B", false), ("C", true), ("D", false)]));

        let mut accounts = vec![account("x", false)];
        let shown = |minutes: i64| {
            let mut accounts = accounts.clone();
            let p = peers([(file("p", "P", at(-minutes), vec![seat("x")]), None)], "me", now);
            attach(&mut accounts, &p, now);
            accounts[0].remote.first().map(|r| r.live)
        };
        assert_eq!(shown(5), Some(true));
        assert_eq!(shown(3 * 60), Some(false), "dimmed: seen 3h ago");
        assert_eq!(shown(25 * 60), None, "older than a day is not shown");
        attach(&mut accounts, &[], now);
        assert!(accounts[0].remote.is_empty());
    }

    #[test]
    fn future_timestamps_are_clamped() {
        let now = at(0);
        // Slightly ahead: treated as now.
        let p = peers([(file("a", "A", at(3), vec![]), None)], "me", now);
        assert_eq!(p[0].seen_at, now);
        // Far ahead: the file's modification time decides, so a dead machine goes stale.
        let p = peers([(file("a", "A", at(600), vec![]), Some(at(-120)))], "me", now);
        assert_eq!(p[0].seen_at, at(-120));
        assert!(!p[0].live);
        let p = peers([(file("a", "A", at(600), vec![]), None)], "me", now);
        assert!(p.is_empty(), "no trustworthy time at all");
    }

    #[test]
    fn writes_only_hashes_and_round_trips_through_the_folder() {
        let dir_ = TempDir::new();
        let now = at(0);
        let mut accounts = vec![account("synthetic-user:synthetic-org", true), account("token:abcdef", true), account("idle", false)];
        accounts[0].email = Some("someone@example.invalid".into());
        let mine = own("11111111-1111-4111-8111-111111111111", "BACHELOR", 300, &accounts, now);
        assert_eq!(mine.seats.len(), 1, "token: logins and idle accounts are left out");
        write(&dir_.0, &mine).unwrap();
        let other = own("22222222-2222-4222-8222-222222222222", "LAPTOP-2", 300, &accounts, now);
        write(&dir_.0, &other).unwrap();
        // A sync client's conflict copy of the other machine, older.
        let stale = Presence { written_at: at(-60), seats: vec![], ..other.clone() };
        std::fs::write(dir(&dir_.0).join("22222222-2222-4222-8222-222222222222 (1).json"), serde_json::to_vec(&stale).unwrap()).unwrap();

        for entry in std::fs::read_dir(dir(&dir_.0)).unwrap() {
            let text = std::fs::read_to_string(entry.unwrap().path()).unwrap();
            assert!(!text.contains('@'), "no emails: {text}");
            assert!(!text.contains("synthetic-user") && !text.contains("Synthetic Org") && !text.contains("abcdef"), "{text}");
        }

        let peers = read(&dir_.0, &mine.machine_id, now);
        assert_eq!(peers.len(), 1);
        let mut idle = vec![account("synthetic-user:synthetic-org", false)];
        attach(&mut idle, &peers, now);
        assert_eq!(idle[0].remote.len(), 1);
        assert_eq!(idle[0].remote[0].machine, "LAPTOP-2");
        assert_eq!(idle[0].remote[0].where_, vec!["Windows", "WSL: Ubuntu"]);
        assert!(idle[0].remote[0].live);
        assert!(!idle[0].active, "another machine never makes an account active here");

        // A clean quit empties the seats.
        write(&dir_.0, &own(&other.machine_id, "LAPTOP-2", 300, &[], now + Duration::minutes(1))).unwrap();
        let peers = read(&dir_.0, &mine.machine_id, now + Duration::minutes(1));
        attach(&mut idle, &peers, now);
        assert!(idle[0].remote.is_empty());
    }

    fn reading(a: &mut AccountUsage, minute: i64, percent: f64, resets: i64) {
        a.fetched_at = Some(at(minute));
        a.windows = vec![UsageWindow { kind: "session".into(), window_seconds: Some(18000), used_percent: percent, resets_at: Some(at(resets)) }];
    }

    #[test]
    fn inference_flags_a_rise_and_clears_after_two_intervals() {
        let hold = Duration::minutes(10) + Duration::seconds(30);
        let mut tracks = Tracks::new();
        let mut list = vec![account("x", false)];
        reading(&mut list[0], 0, 10.0, 300);
        infer(&mut tracks, &mut list, hold, at(0));
        assert!(!list[0].used_outside, "one reading is not a rise");
        reading(&mut list[0], 5, 10.5, 300);
        infer(&mut tracks, &mut list, hold, at(5));
        assert!(!list[0].used_outside, "under one point");
        reading(&mut list[0], 10, 12.0, 300);
        infer(&mut tracks, &mut list, hold, at(10));
        assert!(list[0].used_outside);
        reading(&mut list[0], 15, 12.0, 300);
        infer(&mut tracks, &mut list, hold, at(15));
        assert!(list[0].used_outside, "still within two intervals");
        reading(&mut list[0], 21, 12.0, 300);
        infer(&mut tracks, &mut list, hold, at(21));
        assert!(!list[0].used_outside, "cleared after two intervals without a rise");
    }

    #[test]
    fn inference_ignores_what_it_cannot_attribute() {
        let hold = Duration::minutes(11);
        // A new window (different reset time) is not a rise.
        let mut tracks = Tracks::new();
        let mut list = vec![account("x", false)];
        reading(&mut list[0], 0, 80.0, 60);
        infer(&mut tracks, &mut list, hold, at(0));
        reading(&mut list[0], 5, 90.0, 360);
        infer(&mut tracks, &mut list, hold, at(5));
        assert!(!list[0].used_outside);

        // A stale or failed reading in between does not count as consecutive fresh readings.
        reading(&mut list[0], 10, 95.0, 360);
        list[0].stale = true;
        list[0].error = Some("http_500".into());
        infer(&mut tracks, &mut list, hold, at(10));
        assert!(!list[0].used_outside);
        list[0].stale = false;
        list[0].error = None;
        reading(&mut list[0], 15, 99.0, 360);
        infer(&mut tracks, &mut list, hold, at(15));
        assert!(!list[0].used_outside, "the reading before the failure is not compared");
        reading(&mut list[0], 20, 100.0, 360);
        infer(&mut tracks, &mut list, hold, at(20));
        assert!(list[0].used_outside, "two fresh readings in a row again");

        // In use here, or on a live machine that says so: not "outside".
        for claimed in [true, false] {
            let mut tracks = Tracks::new();
            let mut list = vec![account("x", claimed)];
            if !claimed {
                list[0].remote = vec![RemoteSeat { machine: "P".into(), where_: vec![], seen_at: at(0), live: true }];
            }
            reading(&mut list[0], 0, 10.0, 300);
            infer(&mut tracks, &mut list, hold, at(0));
            reading(&mut list[0], 5, 20.0, 300);
            infer(&mut tracks, &mut list, hold, at(5));
            assert!(!list[0].used_outside);
        }

        // Usage from this PC just before switching away is not counted against the next reading.
        let mut tracks = Tracks::new();
        let mut list = vec![account("x", true)];
        reading(&mut list[0], 0, 10.0, 300);
        infer(&mut tracks, &mut list, hold, at(0));
        list[0].active = false;
        list[0].sources.clear();
        reading(&mut list[0], 5, 20.0, 300);
        infer(&mut tracks, &mut list, hold, at(5));
        assert!(!list[0].used_outside);

        // Not monitored: SlopBar only reads it when it is in use, so nothing to infer.
        let mut tracks = Tracks::new();
        let mut list = vec![account("x", false)];
        list[0].monitor_connected = false;
        reading(&mut list[0], 0, 10.0, 300);
        infer(&mut tracks, &mut list, hold, at(0));
        reading(&mut list[0], 5, 20.0, 300);
        infer(&mut tracks, &mut list, hold, at(5));
        assert!(!list[0].used_outside);
    }
}
