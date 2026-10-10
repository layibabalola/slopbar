use super::*;
use crate::test_support::{slot, TempDir};
use std::collections::VecDeque;
use std::sync::Mutex as SyncMutex;

struct Scripted {
    replies: SyncMutex<VecDeque<Result<Reply, Failure>>>,
    refreshes: SyncMutex<Vec<String>>,
}

impl Scripted {
    fn new(replies: impl IntoIterator<Item = Result<Reply, Failure>>) -> Self {
        Self {
            replies: SyncMutex::new(replies.into_iter().collect()),
            refreshes: SyncMutex::new(vec![]),
        }
    }
    fn calls(&self) -> Vec<String> {
        self.refreshes.lock().unwrap().clone()
    }
}

impl Refresh for Scripted {
    async fn refresh(&self, session: &Session) -> Result<Reply, Failure> {
        self.refreshes
            .lock()
            .unwrap()
            .push(session.refresh_token.clone());
        // Allow a competing poll to enter while this simulated network call is in flight.
        tokio::task::yield_now().await;
        self.replies
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected refresh")
    }
}

fn now() -> DateTime<Utc> {
    DateTime::from_timestamp(1_800_000_000, 0).unwrap()
}
fn reply(access: &str, refresh: Option<&str>) -> Result<Reply, Failure> {
    Ok(Reply {
        access_token: access.into(),
        refresh_token: refresh.map(String::from),
        expires_in: Some(3600),
        token_type: Some("Bearer".into()),
    })
}
fn cli() -> Slot {
    slot("account:org", "cli-access", "cli-refresh", 0)
}
fn login() -> Slot {
    slot("account:org", "monitor-access-1", "monitor-refresh-1", 0)
}

#[tokio::test]
async fn inactive_account_renews_repeatedly_across_restart_without_using_cli_session() {
    let dir = TempDir::new();
    let expected = cli();
    let original = serde_json::to_vec(&expected).unwrap();
    let transport = Scripted::new([
        reply("access-2", Some("refresh-2")),
        reply("access-3", Some("refresh-3")),
    ]);
    let monitor = Monitor::new(dir.0.clone());
    monitor.connect(&expected, &login(), &[]).await.unwrap();
    assert_eq!(
        monitor
            .token_with(&expected, &transport, None, now())
            .await
            .unwrap()
            .0,
        "access-2"
    );
    drop(monitor);
    // Neither a recent CLI login nor a running CLI exists. Use the saved rotated token.
    let restarted = Monitor::new(dir.0.clone());
    assert_eq!(
        restarted
            .token_with(
                &expected,
                &transport,
                None,
                now() + chrono::Duration::days(30)
            )
            .await
            .unwrap()
            .0,
        "access-3"
    );
    assert_eq!(transport.calls(), ["monitor-refresh-1", "refresh-2"]);
    assert_eq!(serde_json::to_vec(&expected).unwrap(), original);
    let saved: Session =
        serde_json::from_slice(&private_store::load_secret(&restarted.path(&expected)).unwrap())
            .unwrap();
    assert_eq!(saved.refresh_token, "refresh-3");
}

#[tokio::test]
async fn concurrent_unauthorized_polls_rotate_only_once() {
    let dir = TempDir::new();
    let monitor = Monitor::new(dir.0.clone());
    let mut fresh = login();
    fresh.data["credentials"]["claudeAiOauth"]["expiresAt"] =
        json!((now() + chrono::Duration::hours(1)).timestamp_millis());
    let expected = cli();
    monitor.connect(&expected, &fresh, &[]).await.unwrap();
    let transport = Scripted::new([reply("access-2", Some("refresh-2"))]);
    let (a, b) = tokio::join!(
        monitor.token_with(&expected, &transport, Some("monitor-access-1"), now()),
        monitor.token_with(&expected, &transport, Some("monitor-access-1"), now()),
    );
    assert_eq!(a.unwrap().0, "access-2");
    assert_eq!(b.unwrap().0, "access-2");
    assert_eq!(transport.calls().len(), 1);
}

#[tokio::test]
async fn still_valid_monitor_token_needs_no_refresh() {
    let dir = TempDir::new();
    let monitor = Monitor::new(dir.0.clone());
    let fresh = slot(
        "account:org",
        "monitor-access",
        "monitor-refresh",
        (now() + chrono::Duration::hours(1)).timestamp_millis(),
    );
    monitor.connect(&cli(), &fresh, &[]).await.unwrap();
    let transport = Scripted::new([]);
    assert_eq!(
        monitor
            .token_with(&cli(), &transport, None, now())
            .await
            .unwrap()
            .0,
        "monitor-access"
    );
    assert!(transport.calls().is_empty());
}

#[tokio::test]
async fn rejects_shared_cli_tokens_and_wrong_account_or_provider() {
    let dir = TempDir::new();
    let monitor = Monitor::new(dir.0.clone());
    assert_eq!(
        monitor.connect(&cli(), &cli(), &[]).await,
        Err("monitor_shared_login".into())
    );
    assert_eq!(
        monitor.connect(&cli(), &login(), &[login()]).await,
        Err("monitor_shared_login".into())
    );
    let mut wrong = login();
    wrong.id = "different:org".into();
    assert_eq!(
        monitor.connect(&cli(), &wrong, &[]).await,
        Err("monitor_wrong_account".into())
    );
    wrong.id = cli().id;
    wrong.provider = Provider::Codex;
    assert_eq!(
        monitor.connect(&cli(), &wrong, &[]).await,
        Err("monitor_wrong_account".into())
    );
    assert!(!monitor.connected(&cli()));
}

#[tokio::test]
async fn revoked_and_ambiguous_refreshes_require_reconnect_even_after_restart() {
    for failure in [Failure::Revoked, Failure::Ambiguous] {
        let dir = TempDir::new();
        let monitor = Monitor::new(dir.0.clone());
        monitor.connect(&cli(), &login(), &[]).await.unwrap();
        let transport = Scripted::new([Err(failure)]);
        assert_eq!(
            monitor.token_with(&cli(), &transport, None, now()).await,
            Err("monitor_reconnect".into())
        );
        let restarted = Monitor::new(dir.0.clone());
        assert_eq!(
            restarted
                .token_with(&cli(), &transport, None, now() + chrono::Duration::days(1))
                .await,
            Err("monitor_reconnect".into())
        );
        assert_eq!(transport.calls().len(), 1);
    }
}

#[tokio::test]
async fn rate_limit_and_network_failure_back_off_then_recover() {
    for (failure, delay, code) in [
        (Failure::RateLimited(900), 900, "rate_limited"),
        (Failure::Transient, 300, "monitor_network"),
    ] {
        let dir = TempDir::new();
        let monitor = Monitor::new(dir.0.clone());
        monitor.connect(&cli(), &login(), &[]).await.unwrap();
        let transport = Scripted::new([Err(failure), reply("access-2", None)]);
        assert_eq!(
            monitor.token_with(&cli(), &transport, None, now()).await,
            Err(code.into())
        );
        assert_eq!(
            monitor
                .token_with(
                    &cli(),
                    &transport,
                    None,
                    now() + chrono::Duration::seconds(delay - 1)
                )
                .await,
            Err(code.into())
        );
        assert_eq!(transport.calls().len(), 1);
        assert_eq!(
            monitor
                .token_with(
                    &cli(),
                    &transport,
                    None,
                    now() + chrono::Duration::seconds(delay)
                )
                .await
                .unwrap()
                .0,
            "access-2"
        );
        assert_eq!(transport.calls().len(), 2);
    }
}

#[tokio::test]
async fn failed_disk_write_keeps_rotated_refresh_token_and_never_replays_old_one() {
    let dir = TempDir::new();
    let monitor = Monitor::new(dir.0.clone());
    monitor.connect(&cli(), &login(), &[]).await.unwrap();
    let path = monitor.path(&cli());
    // Force the atomic rename to fail even when tests run as an administrator.
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    let transport = Scripted::new([
        reply("access-2", Some("refresh-2")),
        reply("access-3", Some("refresh-3")),
    ]);
    assert_eq!(
        monitor.token_with(&cli(), &transport, None, now()).await,
        Err("monitor_storage".into())
    );
    assert_eq!(
        monitor.token_with(&cli(), &transport, None, now()).await,
        Err("monitor_storage".into())
    );
    assert_eq!(transport.calls(), ["monitor-refresh-1"]);
    std::fs::remove_dir(&path).unwrap();
    assert_eq!(
        monitor
            .token_with(&cli(), &transport, None, now())
            .await
            .unwrap()
            .0,
        "access-2"
    );
    assert_eq!(
        monitor
            .token_with(&cli(), &transport, None, now() + chrono::Duration::hours(2))
            .await
            .unwrap()
            .0,
        "access-3"
    );
    assert_eq!(transport.calls(), ["monitor-refresh-1", "refresh-2"]);
}

#[tokio::test]
async fn rejection_of_renewed_token_is_terminal_but_old_poll_cannot_invalidate_new_login() {
    let dir = TempDir::new();
    let monitor = Monitor::new(dir.0.clone());
    monitor.connect(&cli(), &login(), &[]).await.unwrap();
    monitor.reject(&cli(), "monitor-access-1").await.unwrap();
    let restarted = Monitor::new(dir.0.clone());
    let transport = Scripted::new([reply("access-2", None)]);
    assert_eq!(
        restarted.token_with(&cli(), &transport, None, now()).await,
        Err("monitor_reconnect".into())
    );
    let fresh = slot("account:org", "new-login", "new-login-refresh", 0);
    restarted.connect(&cli(), &fresh, &[]).await.unwrap();
    restarted.reject(&cli(), "monitor-access-1").await.unwrap();
    assert_eq!(
        restarted
            .token_with(&cli(), &transport, None, now())
            .await
            .unwrap()
            .0,
        "access-2"
    );
    assert_eq!(transport.calls(), ["new-login-refresh"]);
}

#[tokio::test]
async fn codex_renews_from_jwt_expiry_and_preserves_account_routing() {
    let dir = TempDir::new();
    let mut expected = cli();
    expected.provider = Provider::Codex;
    expected.data = json!({"auth":{"tokens":{"access_token":"cli", "refresh_token":"cli-refresh", "account_id":"workspace"}}});
    let mut fresh = expected.clone();
    let jwt = format!(
        "header.{}.signature",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"exp":1}"#)
    );
    fresh.data["auth"]["tokens"]["access_token"] = json!(jwt);
    fresh.data["auth"]["tokens"]["refresh_token"] = json!("codex-monitor-refresh");
    let monitor = Monitor::new(dir.0.clone());
    monitor.connect(&expected, &fresh, &[]).await.unwrap();
    let transport = Scripted::new([reply("codex-access", None)]);
    assert_eq!(
        monitor
            .token_with(&expected, &transport, None, now())
            .await
            .unwrap(),
        ("codex-access".into(), Some("workspace".into()))
    );
    assert_eq!(transport.calls(), ["codex-monitor-refresh"]);
}

#[tokio::test]
async fn mismatched_or_corrupt_persisted_session_is_never_sent() {
    let dir = TempDir::new();
    let monitor = Monitor::new(dir.0.clone());
    monitor.connect(&cli(), &login(), &[]).await.unwrap();
    let mut wrong = cli();
    wrong.id = "other:org".into();
    std::fs::copy(monitor.path(&cli()), monitor.path(&wrong)).unwrap();
    let transport = Scripted::new([]);
    assert_eq!(
        monitor.token_with(&wrong, &transport, None, now()).await,
        Err("monitor_wrong_account".into())
    );
    std::fs::write(monitor.path(&wrong), b"corrupt").unwrap();
    assert_eq!(
        monitor.token_with(&wrong, &transport, None, now()).await,
        Err("monitor_storage".into())
    );
    assert!(transport.calls().is_empty());
}

#[tokio::test]
async fn removal_deletes_persisted_session_and_in_memory_refresh_token() {
    let dir = TempDir::new();
    let monitor = Monitor::new(dir.0.clone());
    monitor.connect(&cli(), &login(), &[]).await.unwrap();
    monitor.remove(&cli()).await.unwrap();
    assert!(!monitor.connected(&cli()));
    assert_eq!(
        monitor
            .token_with(&cli(), &Scripted::new([]), None, now())
            .await,
        Err("monitor_setup".into())
    );
}
