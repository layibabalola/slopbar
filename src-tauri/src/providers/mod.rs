pub mod claude;
pub mod codex;

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use crate::monitoring::{self, Monitor};

use crate::accounts::{Active, Provider, Slot};
use crate::model::{AccountUsage, ResetGrant, UsageWindow};
use crate::settings::Settings;

/// Last successful numbers per account, used when its saved token can't be used or the usage
/// API asks us to slow down.
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Cached {
    windows: Vec<UsageWindow>,
    credits: Option<String>,
    resets: Vec<ResetGrant>,
    at: Option<DateTime<Utc>>,
    /// Don't call the usage API for this account before this time (after a 429).
    blocked_until: Option<DateTime<Utc>>,
    error: Option<String>,
    monitor_error: Option<String>,
    #[serde(skip)]
    verified: bool,
}

/// An account is never polled more often than this, however often the app refreshes.
const MIN_INTERVAL: i64 = 60;
/// Wait at least this long after a 429, even if the API asks for less.
const MIN_BACKOFF: i64 = 5 * 60;

/// Error code for a 429, carrying the server's Retry-After (seconds) when present.
pub fn rate_limited(resp: &reqwest::Response) -> String {
    let secs = resp
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<i64>().ok())
        .unwrap_or(0);
    format!("rate_limited:{secs}")
}

pub type Cache = HashMap<String, Cached>;

pub fn retry_now(cache: &mut Cache, key: &str) {
    if let Some(c) = cache.get_mut(key) { c.blocked_until = None; c.verified = false; }
}

pub fn load_cache() -> Cache {
    crate::settings::app_dir().and_then(|d| std::fs::read(d.join("usage-cache.json")).ok())
        .and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub fn save_cache(cache: &Cache) -> Result<(), String> {
    let dir = crate::settings::app_dir().ok_or("cache_storage")?;
    let bytes = serde_json::to_vec(cache).map_err(|_| "cache_storage")?;
    crate::private_store::write(&dir.join("usage-cache.json"), &bytes).map_err(|_| "cache_storage".into())
}

pub async fn fetch_all(
    client: &reqwest::Client,
    slots: &[Slot],
    active: &Active,
    cache: &mut Cache,
    settings: &Settings,
    monitor: &Monitor,
) -> Vec<AccountUsage> {
    let enabled = |p: Provider| match p {
        Provider::Claude => settings.providers.claude,
        Provider::Codex => settings.providers.codex,
    };
    let slots: Vec<&Slot> = slots.iter().filter(|s| enabled(s.provider)).collect();
    let now = Utc::now();
    // Skip accounts measured moments ago or backing off after a 429: reuse their numbers.
    let skip = |s: &Slot| {
        cache.get(&monitoring::key(s)).is_some_and(|c| {
            c.blocked_until.is_some_and(|t| t > now)
                || (c.verified && c.error.is_none() && c.at.is_some_and(|t| (now - t).num_seconds() < MIN_INTERVAL))
        })
    };
    let results = futures::future::join_all(slots.iter().map(|s| {
        let skipped = skip(s);
        let is_active = active.contains_key(&(s.provider, s.id.clone()));
        async move { if skipped { None } else { Some(fetch_one(client, s, monitor, is_active).await) } }
    }))
    .await;

    let mut out: Vec<AccountUsage> = slots
        .iter()
        .zip(results)
        .map(|(slot, result)| {
            let sources = active.get(&(slot.provider, slot.id.clone())).cloned().unwrap_or_default();
            let mut acc = AccountUsage {
                id: slot.id.clone(),
                provider: slot.provider,
                email: slot.email.clone(),
                alias: settings.aliases.get(&alias_key(slot.provider, &slot.id)).cloned(),
                plan: slot.plan.clone(),
                org: slot.org.clone(),
                active: !sources.is_empty(),
                sources,
                windows: vec![],
                credits: None,
                resets: vec![],
                error: None,
                monitor_error: None,
                stale: false,
                fetched_at: None,
                monitor_connected: monitor.connected(slot),
            };
            let entry = cache.entry(monitoring::key(slot)).or_default();
            match result {
                Some(Ok(Fetched { windows, credits, resets, monitor_error })) => {
                    *entry = Cached { windows, credits, resets, at: Some(Utc::now()),
                        blocked_until: None, error: None, monitor_error, verified: true };
                }
                Some(Err(err)) => {
                    let backoff = err.strip_prefix("rate_limited:").and_then(|s| s.parse::<i64>().ok())
                        .unwrap_or(MIN_BACKOFF).clamp(MIN_BACKOFF, 86400);
                    entry.blocked_until = Some(Utc::now() + chrono::Duration::seconds(backoff));
                    entry.error = Some(if err.starts_with("rate_limited") { "rate_limited".into() } else { err });
                    entry.monitor_error = None;
                    entry.verified = false;
                }
                None => {},
            }
            acc.windows = entry.windows.clone();
            acc.credits = entry.credits.clone();
            acc.resets = entry.resets.clone();
            acc.fetched_at = entry.at;
            acc.stale = !entry.verified || entry.error.is_some();
            acc.error = entry.error.clone();
            acc.monitor_error = entry.monitor_error.clone();
            acc
        })
        .collect();
    // A stable order: switching accounts must not reshuffle the list.
    out.sort_by_key(|a| (a.provider != Provider::Claude, a.email.clone(), a.id.clone()));
    out
}

pub fn alias_key(provider: Provider, id: &str) -> String {
    format!("{}:{id}", provider.name().to_lowercase())
}

struct Fetched {
    windows: Vec<UsageWindow>,
    credits: Option<String>,
    resets: Vec<ResetGrant>,
    monitor_error: Option<String>,
}

trait Usage {
    async fn fetch(&self, provider: Provider, token: &str, account: Option<&str>) -> Result<Fetched, String>;
}
struct Api<'a>(&'a reqwest::Client);
impl Usage for Api<'_> {
    async fn fetch(&self, provider: Provider, token: &str, account: Option<&str>) -> Result<Fetched, String> {
        fetch_token(self.0, provider, token, account).await
    }
}

async fn fetch_one(client: &reqwest::Client, slot: &Slot, monitor: &Monitor, active: bool) -> Result<Fetched, String> {
    fetch_one_with(client, slot, monitor, active, &Api(client)).await
}

async fn fetch_one_with(client: &reqwest::Client, slot: &Slot, monitor: &Monitor, active: bool, usage: &impl Usage) -> Result<Fetched, String> {
    let result = fetch_session(client, slot, monitor, usage).await;
    // Only the currently captured CLI login may supply a fallback access token.
    // Read usage once; never renew or revoke that login, and never bypass a 429.
    match result {
        Err(error) if active && monitor.connected(slot) && !slot.expired() && !error.starts_with("rate_limited") => {
            if let Some((token, account)) = slot.access() {
                match usage.fetch(slot.provider, &token, account.as_deref()).await {
                    Ok(mut live) => {
                    // Fresh telemetry stays eligible for balancing. Keep the monitor's
                    // separate error visible so its Reconnect control is still honest.
                    live.monitor_error = Some(error);
                    return Ok(live);
                    }
                    Err(error) if error.starts_with("rate_limited") => return Err(error),
                    Err(_) => {},
                }
            }
            Err(error)
        }
        result => result,
    }
}

async fn fetch_session(client: &reqwest::Client, slot: &Slot, monitor: &Monitor, usage: &impl Usage) -> Result<Fetched, String> {
    let owned = monitor.connected(slot);
    let (token, account) = if owned {
        monitor.token(slot, client, None).await?
    } else {
        if slot.expired() { return Err("monitor_setup".into()); }
        slot.access().ok_or("monitor_setup")?
    };
    match usage.fetch(slot.provider, &token, account.as_deref()).await {
        Err(e) if e == "token_invalid" && owned => {
            let (renewed, account) = monitor.token(slot, client, Some(&token)).await?;
            // Exactly one renewal and one usage retry for the independent session.
            match usage.fetch(slot.provider, &renewed, account.as_deref()).await {
                Err(e) if e == "token_invalid" => {
                    monitor.reject(slot, &renewed).await?;
                    Err("monitor_reconnect".into())
                }
                result => result,
            }
        }
        Err(e) if e == "token_invalid" => Err("monitor_setup".into()),
        result => result,
    }
}

async fn fetch_token(client: &reqwest::Client, provider: Provider, token: &str, account: Option<&str>) -> Result<Fetched, String> {
    match provider {
        Provider::Claude => claude::fetch(client, token)
            .await.map(|u| Fetched { windows: u.windows, credits: None, resets: u.resets, monitor_error: None }),
        Provider::Codex => codex::fetch(client, token, account)
            .await.map(|u| Fetched { windows: u.windows, credits: u.credits, resets: u.resets, monitor_error: None }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{slot, TempDir};

    #[tokio::test]
    async fn only_active_cli_access_tokens_can_supply_fresh_usage_after_monitor_failure() {
        struct Observed(std::sync::Mutex<Vec<String>>);
        impl Usage for Observed {
            async fn fetch(&self, _: Provider, token: &str, _: Option<&str>) -> Result<Fetched, String> {
                self.0.lock().unwrap().push(token.to_owned());
                Ok(Fetched { windows: observed(24.0).windows, credits: None, resets: vec![], monitor_error: None })
            }
        }
        for active in [false, true] {
            let dir = TempDir::new();
            let monitor = Monitor::new(dir.0.clone());
            let account = slot("active:org", "live-cli-access", "cli-owned-refresh", i64::MAX);
            let login = slot("active:org", "monitor-access", "monitor-refresh", i64::MAX);
            monitor.connect(&account, &login, &[]).await.unwrap();
            // Persist a rejected monitor, so any attempted automatic renewal fails
            // the test rather than hitting a real provider.
            monitor.token(&account, &reqwest::Client::new(), None).await.unwrap();
            monitor.reject(&account, "monitor-access").await.unwrap();
            let usage = Observed(std::sync::Mutex::new(vec![]));
            let original = serde_json::to_vec(&account).unwrap();
            let result = fetch_one_with(&reqwest::Client::new(), &account, &monitor, active, &usage).await;
            if active {
                let fetched = result.unwrap();
                assert_eq!(fetched.windows[0].used_percent, 24.0);
                assert_eq!(fetched.monitor_error.as_deref(), Some("monitor_reconnect"));
                assert_eq!(*usage.0.lock().unwrap(), ["live-cli-access"]);
            } else {
                assert_eq!(result.err().as_deref(), Some("monitor_reconnect"));
                assert!(usage.0.lock().unwrap().is_empty());
            }
            assert_eq!(serde_json::to_vec(&account).unwrap(), original);
        }
    }

    fn observed(percent: f64) -> Cached {
        Cached {
            windows: vec![UsageWindow { kind: "session".into(), window_seconds: Some(18000), used_percent: percent,
                resets_at: Some(Utc::now() - chrono::Duration::hours(1)) }],
            at: Some(Utc::now() - chrono::Duration::hours(2)), verified: true,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn expired_imported_cli_session_requests_setup_without_refreshing_or_network_calls() {
        let dir = TempDir::new();
        let monitor = Monitor::new(dir.0.clone());
        let account = slot("inactive:org", "expired", "cli-owned-refresh", 0);
        let mut cache = Cache::new();
        let results = fetch_all(&reqwest::Client::new(), &[account], &Active::new(), &mut cache, &Settings::default(), &monitor).await;
        assert_eq!(results[0].error.as_deref(), Some("monitor_setup"));
        assert!(results[0].stale);
        assert!(results[0].windows.is_empty());
        assert!(!results[0].monitor_connected);
        assert!(std::fs::read_dir(&dir.0).unwrap().next().is_none());
    }

    #[tokio::test]
    async fn restart_and_expired_reset_keep_last_observed_usage_instead_of_inventing_zero() {
        let dir = TempDir::new();
        let monitor = Monitor::new(dir.0.clone());
        let account = slot("inactive:org", "expired", "cli-owned-refresh", 0);
        let original = observed(87.0);
        let measured_at = original.at;
        let original_reset = original.windows[0].resets_at;
        let mut before = Cache::new();
        before.insert(monitoring::key(&account), original);
        let bytes = serde_json::to_vec(&before).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("refresh"));
        let mut restarted: Cache = serde_json::from_slice(&bytes).unwrap();
        assert!(!restarted[&monitoring::key(&account)].verified);
        let results = fetch_all(&reqwest::Client::new(), &[account], &Active::new(), &mut restarted, &Settings::default(), &monitor).await;
        assert!(results[0].stale);
        assert_eq!(results[0].windows[0].used_percent, 87.0);
        assert_eq!(results[0].windows[0].resets_at, original_reset);
        assert_eq!(results[0].fetched_at, measured_at);
    }

    #[tokio::test]
    async fn cache_is_provider_scoped_and_backoff_preserves_error_and_timestamp() {
        let dir = TempDir::new();
        let monitor = Monitor::new(dir.0.clone());
        let claude = slot("same-id", "unused", "unused", 0);
        let mut codex = claude.clone();
        codex.provider = Provider::Codex;
        let mut cache = Cache::new();
        for (account, percent) in [(&claude, 87.0), (&codex, 12.0)] {
            let mut entry = observed(percent);
            entry.error = Some("rate_limited".into());
            entry.blocked_until = Some(Utc::now() + chrono::Duration::hours(1));
            cache.insert(monitoring::key(account), entry);
        }
        let results = fetch_all(&reqwest::Client::new(), &[claude, codex], &Active::new(), &mut cache, &Settings::default(), &monitor).await;
        assert_eq!(results[0].windows[0].used_percent, 87.0);
        assert_eq!(results[1].windows[0].used_percent, 12.0);
        assert!(results.iter().all(|a| a.stale && a.error.as_deref() == Some("rate_limited")));
        assert_eq!(cache.len(), 2);
    }
}
