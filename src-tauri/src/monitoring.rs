//! Renewable sessions owned only by the monitor. Never rotate a copied CLI refresh token.
use std::collections::HashMap;
use std::path::PathBuf;

use base64::Engine;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::accounts::{Provider, Slot};
use crate::private_store;

const CLAUDE_TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const CLAUDE_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const CODEX_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

pub fn key(slot: &Slot) -> String {
    format!("{}:{}", slot.provider.name(), slot.id)
}

// Deliberately no Debug: token values must not enter logs or UI errors.
#[derive(Clone, Serialize, Deserialize)]
struct Session {
    version: u8,
    provider: Provider,
    id: String,
    access_token: String,
    refresh_token: String,
    account_id: Option<String>,
    expires_at: Option<DateTime<Utc>>,
    #[serde(default)]
    reconnect: bool,
}

struct State {
    session: Session,
    // Keep a rotated token in memory if its disk write fails. Retry the write before any
    // subsequent network request, rather than refreshing the old on-disk token again.
    dirty: bool,
    retry_at: Option<DateTime<Utc>>,
    error: Option<String>,
}

pub struct Monitor {
    dir: PathBuf,
    // Serializes refresh, reconnect and removal. The desktop is a single-instance app.
    sessions: Mutex<HashMap<String, State>>,
}

impl Monitor {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            sessions: Mutex::new(HashMap::new()),
        }
    }

    fn path(&self, slot: &Slot) -> PathBuf {
        let name = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(key(slot));
        self.dir.join(format!("{name}.session"))
    }

    pub fn connected(&self, slot: &Slot) -> bool {
        self.path(slot).is_file()
    }

    /// A fresh, isolated CLI sign-in is required. Imported/shared CLI sessions are never
    /// promoted to renewable monitoring sessions, even if their access token still works.
    pub async fn connect(
        &self,
        expected: &Slot,
        login: &Slot,
        cli_slots: &[Slot],
    ) -> Result<(), String> {
        if expected.provider != login.provider
            || expected.id != login.id
            || login.id.starts_with("token:")
        {
            return Err("monitor_wrong_account".into());
        }
        let refresh = refresh_token(login)
            .filter(|t| !t.is_empty())
            .ok_or("monitor_reconnect")?;
        if cli_slots
            .iter()
            .chain(std::iter::once(expected))
            .any(|s| s.provider == login.provider && refresh_token(s) == Some(refresh))
        {
            return Err("monitor_shared_login".into());
        }
        let (access, account_id) = login.access().ok_or("monitor_reconnect")?;
        if access.is_empty() {
            return Err("monitor_reconnect".into());
        }
        let session = Session {
            version: 1,
            provider: login.provider,
            id: login.id.clone(),
            access_token: access,
            refresh_token: refresh.to_string(),
            account_id,
            expires_at: expiry(login),
            reconnect: false,
        };
        let mut sessions = self.sessions.lock().await;
        self.save(expected, &session)?;
        sessions.insert(
            key(expected),
            State {
                session,
                dirty: false,
                retry_at: None,
                error: None,
            },
        );
        Ok(())
    }

    fn save(&self, slot: &Slot, session: &Session) -> Result<(), String> {
        let bytes = serde_json::to_vec(session).map_err(|_| "monitor_storage")?;
        private_store::save_secret(&self.path(slot), &bytes).map_err(|_| "monitor_storage".into())
    }

    pub async fn remove(&self, slot: &Slot) -> Result<(), String> {
        let mut sessions = self.sessions.lock().await;
        match std::fs::remove_file(self.path(slot)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err("monitor_storage".into()),
        }
        sessions.remove(&key(slot));
        Ok(())
    }

    pub async fn token(
        &self,
        slot: &Slot,
        client: &reqwest::Client,
        rejected: Option<&str>,
    ) -> Result<(String, Option<String>), String> {
        self.token_with(slot, &Api(client), rejected, Utc::now())
            .await
    }

    /// A freshly renewed token was also rejected. Persist this terminal state so every
    /// poll (and restart) does not rotate another token. Do not invalidate a newer login.
    pub async fn reject(&self, slot: &Slot, token: &str) -> Result<(), String> {
        let mut sessions = self.sessions.lock().await;
        if let Some(state) = sessions.get_mut(&key(slot)) {
            if state.session.access_token == token {
                state.session.reconnect = true;
                state.dirty = true;
                self.save(slot, &state.session)?;
                state.dirty = false;
            }
        }
        Ok(())
    }

    async fn token_with(
        &self,
        slot: &Slot,
        transport: &impl Refresh,
        rejected: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<(String, Option<String>), String> {
        let mut sessions = self.sessions.lock().await;
        let key = key(slot);
        if !sessions.contains_key(&key) {
            let bytes = private_store::load_secret(&self.path(slot)).map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    "monitor_setup"
                } else {
                    "monitor_storage"
                }
            })?;
            let session: Session = serde_json::from_slice(&bytes).map_err(|_| "monitor_storage")?;
            if session.version != 1 || session.provider != slot.provider || session.id != slot.id {
                return Err("monitor_wrong_account".into());
            }
            sessions.insert(
                key.clone(),
                State {
                    session,
                    dirty: false,
                    retry_at: None,
                    error: None,
                },
            );
        }
        let state = sessions.get_mut(&key).unwrap();
        if state.dirty {
            self.save(slot, &state.session)?;
            state.dirty = false;
        }
        if state.session.reconnect {
            return Err("monitor_reconnect".into());
        }
        if state.retry_at.is_some_and(|t| t > now) {
            return Err(state
                .error
                .clone()
                .unwrap_or_else(|| "monitor_network".into()));
        }
        let expired = state
            .session
            .expires_at
            .is_some_and(|t| t <= now + chrono::Duration::seconds(60));
        // If another poll already renewed this token, use that result instead of rotating twice.
        let rejected_current = rejected.is_some_and(|t| t == state.session.access_token);
        if expired || rejected_current {
            match transport.refresh(&state.session).await {
                Ok(reply) => {
                    // Once the server may have rotated, preserve the returned refresh token
                    // even when the rest of the response is invalid. Never retry the old one.
                    if let Some(refresh) = reply.refresh_token.filter(|s| !s.is_empty()) {
                        state.session.refresh_token = refresh;
                    }
                    state.session.access_token = reply.access_token;
                    state.session.expires_at = reply
                        .expires_in
                        .and_then(|n| i64::try_from(n).ok())
                        .and_then(chrono::Duration::try_seconds)
                        .and_then(|d| now.checked_add_signed(d))
                        .or_else(|| jwt_expiry(&state.session.access_token));
                    state.session.reconnect = state.session.access_token.is_empty()
                        || reply
                            .token_type
                            .as_deref()
                            .is_some_and(|t| !t.eq_ignore_ascii_case("bearer"))
                        || state.session.expires_at.is_some_and(|t| t <= now);
                    state.dirty = true;
                    self.save(slot, &state.session)?;
                    state.dirty = false;
                    state.retry_at = None;
                    state.error = None;
                    if state.session.reconnect {
                        return Err("monitor_reconnect".into());
                    }
                }
                Err(Failure::Revoked) => {
                    state.session.reconnect = true;
                    state.dirty = true;
                    self.save(slot, &state.session)?;
                    state.dirty = false;
                    return Err("monitor_reconnect".into());
                }
                Err(failure) => {
                    let (code, delay) = match failure {
                        Failure::RateLimited(n) => ("rate_limited", n.clamp(300, 86400)),
                        // An ambiguous response may have rotated a token. Require reconnect,
                        // rather than replaying a possibly spent refresh token.
                        Failure::Ambiguous => {
                            state.session.reconnect = true;
                            state.dirty = true;
                            self.save(slot, &state.session)?;
                            state.dirty = false;
                            return Err("monitor_reconnect".into());
                        }
                        _ => ("monitor_network", 300),
                    };
                    state.retry_at = Some(now + chrono::Duration::seconds(delay));
                    state.error = Some(code.into());
                    return Err(code.into());
                }
            }
        }
        Ok((
            state.session.access_token.clone(),
            state.session.account_id.clone(),
        ))
    }
}

fn refresh_token(slot: &Slot) -> Option<&str> {
    match slot.provider {
        Provider::Claude => slot.data["credentials"]["claudeAiOauth"]["refreshToken"].as_str(),
        Provider::Codex => slot.data["auth"]["tokens"]["refresh_token"].as_str(),
    }
}

fn expiry(slot: &Slot) -> Option<DateTime<Utc>> {
    match slot.provider {
        Provider::Claude => slot.data["credentials"]["claudeAiOauth"]["expiresAt"]
            .as_i64()
            .and_then(DateTime::from_timestamp_millis),
        Provider::Codex => slot.access().and_then(|(token, _)| jwt_expiry(&token)),
    }
}

fn jwt_expiry(token: &str) -> Option<DateTime<Utc>> {
    let data = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(token.split('.').nth(1)?.trim_end_matches('='))
        .ok()?;
    let claims: Value = serde_json::from_slice(&data).ok()?;
    DateTime::from_timestamp(claims["exp"].as_i64()?, 0)
}

#[derive(Deserialize)]
struct Reply {
    #[serde(default)]
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
    token_type: Option<String>,
}

enum Failure {
    Revoked,
    RateLimited(i64),
    Transient,
    Ambiguous,
}

trait Refresh {
    async fn refresh(&self, session: &Session) -> Result<Reply, Failure>;
}
struct Api<'a>(&'a reqwest::Client);

impl Refresh for Api<'_> {
    async fn refresh(&self, session: &Session) -> Result<Reply, Failure> {
        let (url, client_id) = match session.provider {
            Provider::Claude => (CLAUDE_TOKEN_URL, CLAUDE_CLIENT_ID),
            Provider::Codex => (CODEX_TOKEN_URL, CODEX_CLIENT_ID),
        };
        let request = self.0.post(url).header("Accept", "application/json");
        let fields = [
            ("grant_type", "refresh_token"),
            ("refresh_token", session.refresh_token.as_str()),
            ("client_id", client_id),
        ];
        let request = match session.provider {
            Provider::Claude => request.json(&json!({ "grant_type": "refresh_token", "refresh_token": session.refresh_token, "client_id": client_id })),
            Provider::Codex => request.form(&fields),
        };
        let response = request.send().await.map_err(|e| {
            if e.is_connect() {
                Failure::Transient
            } else {
                Failure::Ambiguous
            }
        })?;
        let status = response.status().as_u16();
        if status == 429 {
            let seconds = response
                .headers()
                .get("retry-after")
                .and_then(|h| h.to_str().ok())
                .and_then(|s| s.parse().ok())
                .unwrap_or(300);
            return Err(Failure::RateLimited(seconds));
        }
        if status >= 500 {
            return Err(Failure::Transient);
        }
        if !response.status().is_success() {
            // Only explicit OAuth rejection is terminal. Do not expose provider response bodies.
            let body: Value = response.json().await.unwrap_or(Value::Null);
            return Err(
                if status == 401 || body["error"].as_str() == Some("invalid_grant") {
                    Failure::Revoked
                } else {
                    Failure::Transient
                },
            );
        }
        response.json().await.map_err(|_| Failure::Ambiguous)
    }
}

#[cfg(test)]
mod tests;
