//! A local door into the running app, so agents can export diagnostics without driving the UI.
//!
//! The running app listens on a per-user named pipe and answers exactly one request: write a
//! diagnostics export (the same allowlisted report as Settings → Data) and return its path.
//! Nothing else crosses the pipe: no usage numbers, identities, credentials or account actions.
//! Remote clients are rejected and the pipe's ACL admits only its owner and SYSTEM.

use serde::{Deserialize, Serialize};

/// Bumped when the request or reply shape changes.
pub const VERSION: u32 = 1;
/// Requests are one short JSON line; anything longer is not a door request.
const MAX_REQUEST: usize = 1024;

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    ExportDiagnostics,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct Reply {
    pub door: u32,
    pub path: Option<String>,
    pub bytes: Option<u64>,
    pub error: Option<String>,
}

/// The pipe for this user's app data: two Windows users never share one.
pub fn pipe_name() -> String {
    let dir = crate::settings::app_dir().map(|d| d.to_string_lossy().to_lowercase()).unwrap_or_default();
    // FNV-1a: stable across builds, unlike std's hasher.
    let hash = dir.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3));
    format!(r"\\.\pipe\slopbar-door-{hash:016x}")
}

fn reply(result: Result<std::path::PathBuf, String>) -> Reply {
    match result {
        Ok(path) => Reply {
            door: VERSION,
            bytes: std::fs::metadata(&path).ok().map(|m| m.len()),
            path: Some(path.to_string_lossy().into_owned()),
            error: None,
        },
        Err(error) => Reply { door: VERSION, error: Some(error), ..Default::default() },
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::future::Future;
    use std::io::{BufRead, Write};
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};

    /// Owner and SYSTEM only, protected from inherited ACEs.
    const SDDL: &str = "D:P(A;;GA;;;OW)(A;;GA;;;SY)";

    fn create(name: &str, first: bool) -> std::io::Result<NamedPipeServer> {
        use windows_sys::Win32::Foundation::LocalFree;
        use windows_sys::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
        use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
        let sddl: Vec<u16> = SDDL.encode_utf16().chain([0]).collect();
        let mut descriptor = std::ptr::null_mut();
        // SAFETY: `sddl` is NUL-terminated and outlives the call; the descriptor it allocates is
        // freed below after the pipe has copied it.
        let ok = unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut descriptor, std::ptr::null_mut()) };
        if ok == 0 {
            return Err(std::io::Error::last_os_error());
        }
        let mut attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        // SAFETY: `attributes` points at a valid descriptor for the duration of the call.
        let server = unsafe {
            ServerOptions::new()
                .first_pipe_instance(first)
                .reject_remote_clients(true)
                .create_with_security_attributes_raw(name, &mut attributes as *mut _ as *mut _)
        };
        // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW above.
        unsafe { LocalFree(descriptor as _) };
        server
    }

    /// Serves door requests until the process exits. `export` writes one diagnostics export.
    /// Fails at once if another process already owns the pipe name.
    pub async fn serve<F, Fut>(name: String, export: F) -> std::io::Result<()>
    where
        F: Fn() -> Fut + Clone + Send + 'static,
        Fut: Future<Output = Result<std::path::PathBuf, String>> + Send,
    {
        let mut server = create(&name, true)?;
        loop {
            server.connect().await?;
            let connected = std::mem::replace(&mut server, create(&name, false)?);
            let export = export.clone();
            tokio::spawn(async move { let _ = answer(connected, export).await; });
        }
    }

    async fn answer<F, Fut>(pipe: NamedPipeServer, export: F) -> std::io::Result<()>
    where
        F: Fn() -> Fut,
        Fut: Future<Output = Result<std::path::PathBuf, String>>,
    {
        let mut reader = BufReader::new(pipe);
        let mut line = String::new();
        (&mut reader).take(MAX_REQUEST as u64).read_line(&mut line).await?;
        let result = match serde_json::from_str::<Request>(line.trim()) {
            Ok(Request::ExportDiagnostics) => export().await,
            Err(_) => Err("unsupported_request".into()),
        };
        let mut bytes = serde_json::to_vec(&reply(result)).map_err(std::io::Error::other)?;
        bytes.push(b'\n');
        let mut pipe = reader.into_inner();
        pipe.write_all(&bytes).await?;
        pipe.flush().await
    }

    /// Sends one request to the running app and waits for its reply.
    pub fn request(name: &str, request: &Request) -> Result<Reply, String> {
        use windows_sys::Win32::Foundation::ERROR_PIPE_BUSY;
        let mut tries = 0;
        let pipe = loop {
            match std::fs::OpenOptions::new().read(true).write(true).open(name) {
                Ok(pipe) => break pipe,
                Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) && tries < 50 => {
                    tries += 1;
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err("not_running".into()),
                Err(e) => return Err(format!("door_unavailable: {e}")),
            }
        };
        let mut line = serde_json::to_vec(request).map_err(|e| e.to_string())?;
        line.push(b'\n');
        (&pipe).write_all(&line).map_err(|e| format!("door_unavailable: {e}"))?;
        let mut answer = String::new();
        std::io::BufReader::new(&pipe).read_line(&mut answer).map_err(|e| format!("door_unavailable: {e}"))?;
        serde_json::from_str(answer.trim()).map_err(|_| "bad_door_reply".into())
    }
}

#[cfg(windows)]
pub use imp::{request, serve};

#[cfg(not(windows))]
pub fn request(_name: &str, _request: &Request) -> Result<Reply, String> {
    Err("unsupported_platform".into())
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    fn test_pipe() -> String {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        format!(r"\\.\pipe\slopbar-door-test-{}-{}", std::process::id(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    }

    async fn ask(name: String, line: &'static [u8]) -> String {
        tokio::task::spawn_blocking(move || {
            use std::io::{BufRead, Write};
            let pipe = std::fs::OpenOptions::new().read(true).write(true).open(&name).unwrap();
            (&pipe).write_all(line).unwrap();
            let mut answer = String::new();
            std::io::BufReader::new(&pipe).read_line(&mut answer).unwrap();
            answer
        }).await.unwrap()
    }

    #[tokio::test]
    async fn exports_on_request_and_refuses_anything_else() {
        let name = test_pipe();
        let dir = crate::test_support::TempDir::new();
        let file = dir.0.join("export.json");
        std::fs::write(&file, b"{}").unwrap();
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let (counted, path) = (calls.clone(), file.clone());
        tokio::spawn(serve(name.clone(), move || {
            let (counted, path) = (counted.clone(), path.clone());
            async move { counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst); Ok(path) }
        }));
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let n = name.clone();
        let reply = tokio::task::spawn_blocking(move || request(&n, &Request::ExportDiagnostics)).await.unwrap().unwrap();
        assert_eq!(reply.door, VERSION);
        assert_eq!(reply.path.as_deref(), Some(file.to_string_lossy().as_ref()));
        assert_eq!(reply.bytes, Some(2));
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);

        for line in [&b"{\"op\":\"switch_account\",\"id\":\"x\"}\n"[..], b"not json\n"] {
            let answer = ask(name.clone(), line).await;
            let reply: Reply = serde_json::from_str(answer.trim()).unwrap();
            assert_eq!(reply.error.as_deref(), Some("unsupported_request"));
            assert!(reply.path.is_none());
        }
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1, "only the export request exports");
    }

    #[tokio::test]
    async fn a_second_server_cannot_take_over_the_name() {
        let name = test_pipe();
        tokio::spawn(serve(name.clone(), || async { Err::<std::path::PathBuf, _>("unused".to_string()) }));
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let second = serve(name, || async { Err::<std::path::PathBuf, _>("unused".to_string()) }).await;
        assert!(second.is_err());
    }

    #[test]
    fn reports_when_the_app_is_not_running() {
        let reply = request(&test_pipe(), &Request::ExportDiagnostics);
        assert_eq!(reply.unwrap_err(), "not_running");
    }
}
