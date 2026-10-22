//! Windows notifications (toasts). What to say and when is decided by the frontend, which owns
//! the translations (see src/alerts.js); this only shows them under SlopBar's own name and icon.

use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;

#[tauri::command]
pub fn notify(app: AppHandle, title: String, body: String) -> Result<(), String> {
    app.notification().builder().title(title).body(body).show().map_err(|e| e.to_string())
}

/// The unexpected-use notice (see `unexpected`), with two buttons: review the account in
/// Settings > Accounts, or open the provider's own security page. Clicking the toast itself
/// opens Settings too. Signing anything out is left to the user on that page.
#[tauri::command]
pub fn notify_unexpected(app: AppHandle, provider: crate::accounts::Provider, title: String, body: String, review: String, security: String) -> Result<(), String> {
    #[cfg(windows)]
    {
        let identifier = app.config().identifier.clone();
        let url = crate::unexpected::security_page(provider);
        tauri_winrt_notification::Toast::new(&identifier)
            .title(&title)
            .text1(&body)
            .add_button(&review, "review")
            .add_button(&security, "security")
            .on_activated(move |action| {
                if action.as_deref() == Some("security") {
                    let _ = std::process::Command::new("explorer.exe").arg(url).spawn();
                } else {
                    crate::app::show_settings_page(&app, "accounts");
                }
                Ok(())
            })
            .show()
            .map_err(|e| e.to_string())
    }
    #[cfg(not(windows))]
    {
        let _ = (provider, review, security);
        notify(app, title, body)
    }
}

/// Toasts are attributed to an app user model id (the plugin uses the bundle identifier). The
/// installer's Start menu shortcut declares it; registering it here too gives toasts SlopBar's
/// name and icon whatever way the app was started.
#[cfg(windows)]
pub fn register_app_id(identifier: &str) {
    use windows_sys::Win32::System::Registry::{RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ};
    let Some(dir) = crate::settings::app_dir() else { return };
    let icon = dir.join("icon.png");
    if std::fs::create_dir_all(&dir).is_err() || std::fs::write(&icon, include_bytes!("../icons/128x128@2x.png")).is_err() {
        return;
    }
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let key = wide(&format!(r"Software\Classes\AppUserModelId\{identifier}"));
    for (name, value) in [("DisplayName", "SlopBar".to_string()), ("IconUri", icon.display().to_string())] {
        let data = wide(&value);
        unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                wide(name).as_ptr(),
                REG_SZ,
                data.as_ptr() as *const _,
                (data.len() * 2) as u32,
            );
        }
    }
}
