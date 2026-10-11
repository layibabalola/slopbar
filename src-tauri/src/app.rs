use crate::{accounts, balancer, dock, migrate, model, monitoring, notify, panel, providers, settings, sources, updates};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::{AppHandle, Emitter, Manager, WindowEvent, Wry};
use tauri_plugin_autostart::ManagerExt;

use accounts::Provider;
use model::Snapshot;
use settings::Settings;

const SETTINGS_LABEL: &str = "settings";
const MENU_IDS: [&str; 3] = ["refresh", "settings", "quit"];

pub struct AppState {
    monitor: monitoring::Monitor,
    refresh_lock: tokio::sync::Mutex<()>,
    account_operations: monitoring::Operations,
    pub(crate) snapshot: Mutex<Snapshot>,
    pub(crate) settings: Mutex<Settings>,
    /// The widget's right-click menu and its items (relabeled when the language changes).
    pub(crate) menu: Mutex<Option<Menu<Wry>>>,
    pub(crate) menu_items: Mutex<Vec<MenuItem<Wry>>>,
    /// Physical center of the widget when it was last clicked; the flyout opens above it.
    pub(crate) panel_anchor: Mutex<Option<(f64, f64)>>,
    pub(crate) panel_height: Mutex<f64>,
    pub(crate) panel_hidden_at: Mutex<Option<Instant>>,
    /// Content width of the taskbar widget, in CSS px.
    pub(crate) dock_width: Mutex<f64>,
    /// Last known usage per account, for accounts whose saved token can't be used right now.
    pub(crate) usage_cache: Mutex<providers::Cache>,
    /// When each provider was last switched automatically (the balancer's cooldown).
    pub(crate) last_switch: Mutex<HashMap<Provider, Instant>>,
    /// Sources found by the last refresh, where switches are written.
    pub(crate) sources: Mutex<Vec<sources::Source>>,
}

#[tauri::command]
fn get_usage(state: tauri::State<AppState>) -> Snapshot {
    state.snapshot.lock().unwrap().clone()
}

#[tauri::command]
async fn refresh(app: AppHandle) -> Snapshot {
    do_refresh(&app).await
}

#[derive(Clone, Serialize)]
struct Switched {
    provider: Provider,
    from: Option<String>,
    to: Option<String>,
    auto: bool,
}

/// Display name of an account: its alias, else its email.
fn account_label(app: &AppHandle, provider: Provider, id: &str) -> Option<String> {
    let snapshot = app.state::<AppState>().snapshot.lock().unwrap().clone();
    snapshot
        .accounts
        .iter()
        .find(|a| a.provider == provider && a.id == id)
        .and_then(|a| a.alias.clone().or(a.email.clone()))
}

async fn switch_to(app: &AppHandle, provider: Provider, id: String, auto: bool) -> Result<(), String> {
    let found = app.state::<AppState>().sources.lock().unwrap().clone();
    let from = {
        let snapshot = app.state::<AppState>().snapshot.lock().unwrap().clone();
        snapshot.accounts.iter().find(|a| a.provider == provider && a.active).map(|a| a.id.clone())
    };
    let target = id.clone();
    tauri::async_runtime::spawn_blocking(move || accounts::switch(&found, provider, &target))
        .await
        .map_err(|e| e.to_string())??;
    let event = Switched {
        provider,
        from: from.and_then(|f| account_label(app, provider, &f)),
        to: account_label(app, provider, &id),
        auto,
    };
    let _ = app.emit("account-switched", event);
    Ok(())
}

#[tauri::command]
async fn switch_account(app: AppHandle, provider: Provider, id: String) -> Result<Snapshot, String> {
    switch_to(&app, provider, id, false).await?;
    Ok(do_refresh(&app).await)
}

/// Opens a terminal running the CLI's own sign-in; the account appears once it finishes.
#[tauri::command]
async fn add_account(app: AppHandle, provider: Provider) -> Result<(), String> {
    let found = {
        let known = app.state::<AppState>().sources.lock().unwrap().clone();
        if known.is_empty() {
            tauri::async_runtime::spawn_blocking(sources::discover).await.unwrap_or_default()
        } else {
            known
        }
    };
    // Probing for the CLI runs wsl.exe / where.exe; keep it off the async runtime.
    let job = tauri::async_runtime::spawn_blocking(move || accounts::start_login(provider, &found))
        .await
        .map_err(|e| e.to_string())??;
    tauri::async_runtime::spawn(async move {
        let added = job.wait(&http_client()).await;
        if let Some(slot) = added {
            let _ = app.emit("account-added", (slot.provider, slot.email));
            do_refresh(&app).await;
        }
    });
    Ok(())
}

/// A one-time login owns its own refresh-token chain; it never changes the active CLI.
#[tauri::command]
async fn connect_monitoring(app: AppHandle, provider: Provider, id: String) -> Result<(), String> {
    let operation = app.state::<AppState>().account_operations.begin(provider, &id)?;
    let expected = accounts::load_slots().into_iter().find(|s| s.provider == provider && s.id == id)
        .ok_or("monitor_wrong_account")?;
    let key = monitoring::key(&expected);
    let found = app.state::<AppState>().sources.lock().unwrap().clone();
    let job = tauri::async_runtime::spawn_blocking(move || accounts::start_login(provider, &found)).await;
    let job = match job {
        Ok(Ok(job)) => job,
        _ => {
            return Err("monitor_login_failed".into());
        }
    };
    tauri::async_runtime::spawn(async move {
        let operation = operation;
        let outcome = match job.wait_isolated(&http_client()).await {
            Some(login) => {
                let slots = accounts::load_slots();
                if slots.iter().any(|s| s.provider == provider && s.id == id) {
                    app.state::<AppState>().monitor.connect_replacing(&expected, &login, &slots, &http_client()).await
                } else { Err("monitor_wrong_account".into()) }
            }
            None => Err("monitor_login_failed".into()),
        };
        drop(operation);
        let _ = app.emit("monitoring-login", serde_json::json!({ "provider": provider, "id": id, "error": outcome.err() }));
        // Preserve last-known data, but allow the new login to bypass an earlier error's backoff.
        {
            let state = app.state::<AppState>();
            let _refresh = state.refresh_lock.lock().await;
            providers::retry_now(&mut state.usage_cache.lock().unwrap(), &key);
        }
        do_refresh(&app).await;
    });
    Ok(())
}

#[tauri::command]
async fn remove_account(app: AppHandle, provider: Provider, id: String) -> Result<Snapshot, String> {
    let operation = app.state::<AppState>().account_operations.begin(provider, &id)?;
    let active = {
        let snapshot = app.state::<AppState>().snapshot.lock().unwrap().clone();
        snapshot.accounts.iter().any(|a| a.provider == provider && a.id == id && a.active)
    };
    if active {
        // It would be captured again from the CLI on the next refresh.
        return Err("active".into());
    }
    if let Some(slot) = accounts::load_slots().into_iter().find(|s| s.provider == provider && s.id == id) {
        app.state::<AppState>().monitor.disconnect(&slot, &http_client()).await?;
        app.state::<AppState>().usage_cache.lock().unwrap().remove(&monitoring::key(&slot));
    }
    accounts::remove_slot(provider, &id).map_err(|e| e.to_string())?;
    drop(operation);
    Ok(do_refresh(&app).await)
}

#[derive(Serialize)]
struct Disconnected { snapshot: Snapshot, revoked: bool }

#[tauri::command]
async fn disconnect_monitoring(app: AppHandle, provider: Provider, id: String) -> Result<Disconnected, String> {
    let operation = app.state::<AppState>().account_operations.begin(provider, &id)?;
    let slot = accounts::load_slots().into_iter().find(|s| s.provider == provider && s.id == id)
        .ok_or("monitor_wrong_account")?;
    let revoked = app.state::<AppState>().monitor.disconnect(&slot, &http_client()).await?;
    {
        let state = app.state::<AppState>();
        let _refresh = state.refresh_lock.lock().await;
        providers::retry_now(&mut state.usage_cache.lock().unwrap(), &monitoring::key(&slot));
    }
    drop(operation);
    Ok(Disconnected { snapshot: do_refresh(&app).await, revoked })
}

#[tauri::command]
async fn check_for_updates(app: AppHandle) -> Result<updates::UpdateInfo, String> {
    updates::check(&app).await
}

#[tauri::command]
async fn install_update(app: AppHandle) -> Result<(), String> {
    updates::install(&app).await
}

#[tauri::command]
fn app_version(app: AppHandle) -> String {
    let version = app.package_info().version.to_string();
    if cfg!(feature = "upstream-updates") { version } else { format!("{version} (independent monitoring)") }
}

#[tauri::command]
fn updates_enabled() -> bool { cfg!(feature = "upstream-updates") }

/// The OS display language (WebView2's navigator.languages doesn't follow it reliably).
#[tauri::command]
fn system_locale() -> Option<String> {
    sys_locale::get_locale()
}

#[tauri::command]
fn get_settings(state: tauri::State<AppState>) -> Settings {
    state.settings.lock().unwrap().clone()
}

#[tauri::command]
async fn save_settings(app: AppHandle, settings: Settings) -> Result<(), String> {
    settings::save(&settings)?;
    let previous = std::mem::replace(&mut *app.state::<AppState>().settings.lock().unwrap(), settings.clone());
    apply_settings(&app);
    let providers_changed = previous.providers.claude != settings.providers.claude
        || previous.providers.codex != settings.providers.codex;
    if providers_changed {
        do_refresh(&app).await;
    }
    Ok(())
}

#[tauri::command]
fn open_settings(app: AppHandle) {
    show_settings(&app);
}

#[tauri::command]
fn set_panel_height(app: AppHandle, height: f64) {
    *app.state::<AppState>().panel_height.lock().unwrap() = height;
    panel::place(&app);
}

#[tauri::command]
fn set_dock_width(app: AppHandle, width: f64) {
    *app.state::<AppState>().dock_width.lock().unwrap() = width;
}

#[tauri::command]
fn dock_clicked(app: AppHandle) {
    if let Some(w) = app.get_webview_window(dock::LABEL) {
        if let (Ok(pos), Ok(size)) = (w.outer_position(), w.outer_size()) {
            *app.state::<AppState>().panel_anchor.lock().unwrap() = Some((
                pos.x as f64 + size.width as f64 / 2.0,
                pos.y as f64 + size.height as f64 / 2.0,
            ));
        }
    }
    panel::toggle(&app);
}

/// The Windows accent color as WinUI uses it on light and dark backgrounds
/// (SystemAccentColorDark1 and SystemAccentColorLight2), from the system's accent palette.
#[tauri::command]
fn accent_colors() -> Option<(String, String)> {
    let p = dock::accent_palette()?;
    let hex = |i: usize| format!("#{:02x}{:02x}{:02x}", p[i * 4], p[i * 4 + 1], p[i * 4 + 2]);
    // Palette entries: Light3, Light2, Light1, Accent, Dark1, Dark2, Dark3, (unused).
    Some((hex(4), hex(1)))
}

/// Whether the settings window is on Mica (Windows 11); elsewhere it paints its own background.
#[tauri::command]
fn has_mica() -> bool {
    dock::is_windows_11()
}

#[tauri::command]
fn taskbar_theme() -> &'static str {
    if dock::taskbar_is_light() { "light" } else { "dark" }
}

/// Right-click on the widget: the native context menu, like the built-in tray items have.
#[tauri::command]
fn dock_menu(app: AppHandle) {
    let menu = app.state::<AppState>().menu.lock().unwrap().clone();
    if let (Some(menu), Some(w)) = (menu, app.get_webview_window(dock::LABEL)) {
        let _ = w.popup_menu(&menu);
    }
}

/// Menu labels come from the frontend, which owns the translations.
#[tauri::command]
fn set_menu_labels(state: tauri::State<AppState>, labels: HashMap<String, String>) {
    for item in state.menu_items.lock().unwrap().iter() {
        if let Some(text) = labels.get(item.id().as_ref()) {
            let _ = item.set_text(text);
        }
    }
}

fn show_settings(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(SETTINGS_LABEL) {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// Applies the settings that live outside the webviews.
pub(crate) fn apply_settings(app: &AppHandle) {
    let settings = app.state::<AppState>().settings.lock().unwrap().clone();
    let autolaunch = app.autolaunch();
    if autolaunch.is_enabled().unwrap_or(false) != settings.launch_at_login {
        let _ = if settings.launch_at_login { autolaunch.enable() } else { autolaunch.disable() };
    }
    let _ = app.emit("settings-changed", &settings);
}

fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("http client")
}

/// Captures live logins into the vault, fetches every account's usage, then lets the balancer
/// switch accounts that reached their threshold.
async fn do_refresh(app: &AppHandle) -> Snapshot {
    let state = app.state::<AppState>();
    let _refresh = state.refresh_lock.lock().await;
    let settings = state.settings.lock().unwrap().clone();
    let found = tauri::async_runtime::spawn_blocking(sources::discover)
        .await
        .unwrap_or_default();
    *state.sources.lock().unwrap() = found.clone();
    static CLEANUP_STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    accounts::resolve_identities(&http_client(), &found).await;
    let scan = found.clone();
    let (active, slots) = tauri::async_runtime::spawn_blocking(move || (accounts::capture(&scan), accounts::load_slots()))
        .await
        .unwrap_or_default();
    if !CLEANUP_STARTED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        let homes = found.clone();
        let protected = state.monitor.protected_tokens(&slots).await;
        let _ = tauri::async_runtime::spawn_blocking(move || crate::login::cleanup(&homes, protected.as_ref())).await;
    }

    let mut cache = state.usage_cache.lock().unwrap().clone();
    let mut list = providers::fetch_all(&http_client(), &slots, &active, &mut cache, &settings, &state.monitor).await;
    let valid: HashSet<_> = slots.iter().map(monitoring::key).collect();
    cache.retain(|key, _| valid.contains(key));
    let _ = providers::save_cache(&cache);
    *state.usage_cache.lock().unwrap() = cache;

    let decisions = {
        let last = state.last_switch.lock().unwrap().clone();
        balancer::decide(&list, &settings, &last)
    };
    for (provider, id) in decisions {
        // Publish the current numbers first so the switch notice can name both accounts.
        *state.snapshot.lock().unwrap() = Snapshot { accounts: list.clone(), updated_at: Some(chrono::Utc::now()) };
        if switch_to(app, provider, id.clone(), true).await.is_ok() {
            state.last_switch.lock().unwrap().insert(provider, Instant::now());
            for a in list.iter_mut().filter(|a| a.provider == provider) {
                let now_active = a.id == id;
                if now_active != a.active {
                    a.sources = if now_active {
                        found.iter().map(|s| s.label.clone()).collect()
                    } else {
                        vec![]
                    };
                    a.active = now_active;
                }
            }
        }
    }

    let snapshot = Snapshot { accounts: list, updated_at: Some(chrono::Utc::now()) };
    *state.snapshot.lock().unwrap() = snapshot.clone();
    let _ = app.emit("usage-updated", &snapshot);
    snapshot
}

/// Refreshes on the interval from settings, re-reading it so changes apply without a restart.
async fn refresh_loop(app: AppHandle) {
    loop {
        do_refresh(&app).await;
        let started = Instant::now();
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let minutes = app.state::<AppState>().settings.lock().unwrap().refresh_minutes.max(1);
            if started.elapsed() >= Duration::from_secs(minutes as u64 * 60) {
                break;
            }
        }
    }
}

pub fn run() {
    migrate::run();
    // Managed before the builder runs: config windows load (and call commands) before `setup`.
    let state = AppState {
        snapshot: Mutex::default(),
        settings: Mutex::new(settings::load()),
        menu: Mutex::default(),
        menu_items: Mutex::default(),
        panel_anchor: Mutex::default(),
        panel_height: Mutex::new(320.0),
        panel_hidden_at: Mutex::default(),
        dock_width: Mutex::new(160.0),
        usage_cache: Mutex::new(providers::load_cache()),
        monitor: monitoring::Monitor::new(settings::app_dir().expect("config directory").join("monitoring")),
        refresh_lock: tokio::sync::Mutex::new(()),
        account_operations: monitoring::Operations::default(),
        last_switch: Mutex::default(),
        sources: Mutex::default(),
    };
    tauri::Builder::default()
        // Must come first: a second launch hands over to the running instance and exits.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_settings(app)))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_notification::init())
        .manage(state)
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .invoke_handler(tauri::generate_handler![
            get_usage,
            refresh,
            get_settings,
            system_locale,
            check_for_updates,
            install_update,
            app_version,
            updates_enabled,
            save_settings,
            open_settings,
            set_panel_height,
            set_menu_labels,
            switch_account,
            add_account,
            connect_monitoring,
            disconnect_monitoring,
            remove_account,
            set_dock_width,
            dock_clicked,
            dock_menu,
            accent_colors,
            notify::notify,
            has_mica,
            taskbar_theme
        ])
        .setup(|app| {
            let handle = app.handle();
            #[cfg(windows)]
            notify::register_app_id(&app.config().identifier);

            let defaults = ["Refresh", "Settings…", "Quit"];
            let items = MENU_IDS
                .iter()
                .zip(defaults)
                .map(|(id, text)| MenuItem::with_id(app, *id, text, true, None::<&str>))
                .collect::<Result<Vec<_>, _>>()?;
            let separator = PredefinedMenuItem::separator(app)?;
            let menu = Menu::with_items(app, &[&items[0], &items[1], &separator, &items[2]])?;
            *app.state::<AppState>().menu.lock().unwrap() = Some(menu);
            *app.state::<AppState>().menu_items.lock().unwrap() = items;
            app.on_menu_event(|app, event| match event.id().as_ref() {
                "refresh" => {
                    let app = app.clone();
                    tauri::async_runtime::spawn(async move { do_refresh(&app).await });
                }
                "settings" => show_settings(app),
                "quit" => app.exit(0),
                _ => {}
            });

            #[cfg(windows)]
            for label in [dock::LABEL, panel::LABEL] {
                if let Some(win) = app.get_webview_window(label) {
                    dock::exclude_from_peek(&win);
                }
            }
            #[cfg(windows)]
            if let Some(win) = app.get_webview_window(panel::LABEL) {
                panel::round_corners(&win);
            }
            if let Some(win) = app.get_webview_window(panel::LABEL) {
                let handle = handle.clone();
                win.on_window_event(move |e| {
                    if let WindowEvent::Focused(false) = e {
                        panel::on_blur(&handle);
                    }
                });
            }
            if let Some(win) = app.get_webview_window(SETTINGS_LABEL) {
                if dock::is_windows_11() {
                    use tauri::window::{Effect, EffectsBuilder};
                    let _ = win.set_effects(EffectsBuilder::new().effect(Effect::Mica).build());
                }
                let w = win.clone();
                win.on_window_event(move |e| {
                    if let WindowEvent::CloseRequested { api, .. } = e {
                        api.prevent_close();
                        let _ = w.hide();
                    }
                });
            }

            apply_settings(handle);
            dock::show_docked(handle);

            // Updates install silently; after one, say so once in the widget.
            let current = app.package_info().version.to_string();
            let previous = {
                let state = app.state::<AppState>();
                let mut s = state.settings.lock().unwrap();
                let previous = s.last_version.replace(current.clone());
                let _ = settings::save(&s);
                previous
            };
            if previous.is_some_and(|p| p != current) {
                let h = handle.clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(4)).await;
                    let _ = h.emit("update-installed", current);
                });
            }

            let h = handle.clone();
            std::thread::spawn(move || dock::keep_docked(h));
            tauri::async_runtime::spawn(refresh_loop(handle.clone()));
            tauri::async_runtime::spawn(updates::run_loop(handle.clone()));
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running slopbar");
}
