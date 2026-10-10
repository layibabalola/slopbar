use super::*;
use crate::test_support::TempDir;

#[test]
fn authentication_overrides_are_removed_case_insensitively() {
    for name in [
        "CLAUDE_CODE_OAUTH_CLIENT_ID",
        "CLAUDE_CODE_CUSTOM_OAUTH_URL",
        "CLAUDE_CODE_OAUTH_REFRESH_TOKEN",
        "CLAUDE_CODE_OAUTH_REFRESH_TOKEN_SCOPES",
        "CLAUDE_SECURESTORAGE_CONFIG_DIR",
        "CLAUDE_CODE_FORCE_WINDOWS_CREDMAN",
        "ANTHROPIC_API_KEY",
        "CODEX_REFRESH_TOKEN_URL_OVERRIDE",
        "OPENAI_API_KEY",
        "codex_app_server_login_client_id",
        "CLAUDECODE",
        "USE_STAGING_OAUTH",
    ] {
        assert!(blocked_environment(name), "{name}");
    }
    assert!(!blocked_environment("PATH"));
    assert!(!blocked_environment("HTTPS_PROXY"));
}

#[cfg(unix)]
#[test]
fn shell_profile_overrides_do_not_reach_isolated_cli() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new();
    // A quoted directory name exercises the actual WSL script quoting.
    let config = dir.0.join("sign-in's config");
    std::fs::create_dir(&config).unwrap();
    let fake = dir.0.join("claude");
    std::fs::write(&fake, "#!/bin/sh\nenv\n").unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o700)).unwrap();
    let script = format!("export CLAUDE_CODE_OAUTH_CLIENT_ID=wrong CLAUDE_SECURESTORAGE_CONFIG_DIR=/shared CLAUDE_CODE_FORCE_WINDOWS_CREDMAN=1 ANTHROPIC_API_KEY=secret OPENAI_API_KEY=secret CODEX_HOME=/shared; {}",
        shell_script(Provider::Claude, config.to_str().unwrap(), "claude", false));
    let output = Command::new("bash")
        .args(["--noprofile", "--norc", "-c", &script])
        .env("PATH", format!("{}:/usr/bin:/bin", dir.0.display()))
        .output()
        .unwrap();
    assert!(output.status.success());
    let env = String::from_utf8(output.stdout).unwrap();
    assert!(env.contains(&format!("CLAUDE_CONFIG_DIR={}\n", config.display())));
    assert!(env.contains(&format!(
        "CLAUDE_SECURESTORAGE_CONFIG_DIR={}\n",
        config.display()
    )));
    for name in [
        "CLAUDE_CODE_OAUTH_CLIENT_ID=",
        "CLAUDE_CODE_FORCE_WINDOWS_CREDMAN=",
        "ANTHROPIC_API_KEY=",
        "OPENAI_API_KEY=",
        "CODEX_HOME=",
    ] {
        assert!(!env.contains(name), "{name}");
    }
    assert_eq!(
        std::fs::metadata(config).unwrap().permissions().mode() & 0o777,
        0o700
    );
}

#[test]
fn cleanup_eligibility_is_strict_and_failure_pause_preserves_exit_status() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1800);
    assert!(abandoned(".slopbar-login-123-1000-1", now));
    assert!(!abandoned(".slopbar-login-123-1700000-1", now));
    assert!(!abandoned(".slopbar-login-not-ours", now));
    assert!(!abandoned(".slopbar-login-123-1000-1-extra", now));
    assert!(!abandoned(
        ".slopbar-login-123-9999999999999999999999-1",
        now
    ));
    let dir = TempDir::new();
    prepare(&dir.0, Provider::Claude, "claude auth login").unwrap();
    let script = std::fs::read_to_string(dir.0.join("login.cmd")).unwrap();
    assert!(script.contains("call claude auth login"));
    assert!(script.contains("set \"slopbar_login_status=%errorlevel%\"\r\nif not"));
    assert!(script.ends_with("exit /b %slopbar_login_status%\r\n"));
}

#[test]
fn startup_cleanup_keeps_fresh_unmarked_and_linked_directories() {
    let dir = TempDir::new();
    let fresh = dir.0.join(format!(
        ".slopbar-login-1-{}-1",
        chrono::Utc::now().timestamp_millis()
    ));
    std::fs::create_dir(&fresh).unwrap();
    prepare(&fresh, Provider::Claude, "claude auth login").unwrap();
    let unknown = dir.0.join(".slopbar-login-1-1000-2");
    std::fs::create_dir(&unknown).unwrap();
    let old = dir.0.join(".slopbar-login-1-1000-4");
    std::fs::create_dir(&old).unwrap();
    prepare(&old, Provider::Codex, "codex login").unwrap();
    let legacy = dir.0.join(".slopbar-login-1-1000-5");
    std::fs::create_dir(&legacy).unwrap();
    std::fs::write(
        legacy.join(".credentials.json"),
        br#"{"claudeAiOauth":{"accessToken":"synthetic"}}"#,
    )
    .unwrap();
    let source = Source {
        label: "test".into(),
        home: dir.0.clone(),
        native_home: dir.0.to_string_lossy().into_owned(),
        distro: None,
        claude_dir: None,
        codex_dir: None,
    };
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&fresh, dir.0.join(".slopbar-login-1-1000-3")).unwrap();
    }
    let mut logouts = vec![];
    cleanup_with(&[source], |provider, _, path, _| {
        logouts.push((provider, path.to_owned()))
    });
    assert_eq!(logouts.len(), 2);
    assert!(!old.exists());
    assert!(!legacy.exists());
    assert!(fresh.exists());
    assert!(unknown.exists());
    #[cfg(unix)]
    assert!(dir.0.join(".slopbar-login-1-1000-3").is_symlink());
}

#[cfg(windows)]
#[test]
fn closing_windows_job_terminates_the_login_process() {
    use std::os::windows::process::CommandExt;
    let mut child = Command::new("cmd.exe")
        .args(["/d", "/c", "ping -n 30 127.0.0.1 >nul"])
        .creation_flags(0x0800_0000)
        .spawn()
        .unwrap();
    let tree = ProcessTree::attach(&child).unwrap();
    assert!(child.try_wait().unwrap().is_none());
    drop(tree);
    let start = std::time::Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(!status.success());
            break;
        }
        if start.elapsed() > Duration::from_secs(3) {
            let _ = child.kill();
            panic!("job did not terminate login process")
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
