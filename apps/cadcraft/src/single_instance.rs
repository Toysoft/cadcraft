//! One CADCraft per Windows sign-in: a launch that only names files hands them to the CADCraft
//! that is already running, where they open as tabs, and exits.
//!
//! Outlook attachments, Explorer double-clicks and Open With all start `cadcraft.exe "<file>"`, so
//! without single instance, every file got its own process and window. The first CADCraft listens on a Unix
//! domain socket (Windows 10 1803 and later) in `%LOCALAPPDATA%\CADCraft`. The socket is named after the
//! Windows session (`instance-Console.sock`), so two sign-ins of the same user never open files in each
//! other's windows. The exchange is one line each way, a JSON request naming absolute paths and `ok`,
//! and the running app opens the paths exactly as File ▸ Open would.

use std::sync::{Arc, Mutex, PoisonError};
#[cfg(windows)]
use std::time::Duration;

/// Longest request read: one line of JSON, far longer than any real list of paths.
#[cfg_attr(not(windows), allow(dead_code))]
const MAX_REQUEST: u64 = 1 << 20;

/// How long either side waits for the other before giving up.
#[cfg(windows)]
const TIMEOUT: Duration = Duration::from_secs(5);

/// What this launch should do.
#[cfg_attr(not(windows), allow(dead_code))]
pub enum Claim {
    /// The running CADCraft took the files: exit without a window.
    HandedOff,
    /// This is the first CADCraft: open a window and accept later launches' files.
    Primary(Server),
    /// Open a window of its own and accept nothing.
    Alone,
}

/// Files that later launches handed over, and how to wake the UI for them.
#[derive(Default)]
struct Inbox {
    paths: Vec<String>,
    wake: Option<Box<dyn Fn() + Send>>,
}

/// The first CADCraft's end of the socket. A background thread accepts launches until the
/// process exits; the UI picks their files up through [`Server::take_paths`].
#[derive(Clone)]
pub struct Server {
    inbox: Arc<Mutex<Inbox>>,
}

impl Server {
    #[cfg_attr(not(windows), allow(dead_code))]
    fn new() -> Self {
        Self { inbox: Arc::default() }
    }

    /// Queue files from another launch and wake the UI.
    #[cfg_attr(not(windows), allow(dead_code))]
    fn deliver(&self, paths: Vec<String>) {
        let mut inbox = self.inbox.lock().unwrap_or_else(PoisonError::into_inner);
        inbox.paths.extend(paths);
        if let Some(wake) = &inbox.wake {
            wake();
        }
    }

    /// Register a callback to wake the UI when new files arrive.
    pub fn set_wake(&self, wake: impl Fn() + Send + 'static) {
        self.inbox.lock().unwrap_or_else(PoisonError::into_inner).wake = Some(Box::new(wake));
    }

    /// Take any queued paths delivered to this server.
    pub fn take_paths(&self) -> Vec<String> {
        std::mem::take(&mut self.inbox.lock().unwrap_or_else(PoisonError::into_inner).paths)
    }
}

/// The request a launch sends: `{"open":[…]}` and a newline.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn request(paths: &[String]) -> String {
    let mut line = serde_json::json!({ "open": paths }).to_string();
    line.push('\n');
    line
}

/// The paths a request names, or `None` when it isn't a request.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn parse_request(line: &str) -> Option<Vec<String>> {
    let value: serde_json::Value = serde_json::from_str(line.trim_end()).ok()?;
    let open = value.get("open")?.as_array()?;
    let paths = open.iter().map(|p| p.as_str().filter(|s| !s.is_empty()).map(str::to_string)).collect::<Option<Vec<String>>>()?;
    (!paths.is_empty()).then_some(paths)
}

/// File arguments made absolute against this launch's working folder.
pub fn absolute(paths: &[String]) -> Vec<String> {
    paths.iter().map(|p| std::path::absolute(p).map_or_else(|_| p.clone(), |a| a.to_string_lossy().into_owned())).collect()
}

/// Decide what this launch does. `files` are absolute ([`absolute`]); `may_hand_off` is false
/// when the launch asked for anything besides opening files, so it gets its own window.
pub fn claim(files: &[String], may_hand_off: bool) -> Claim {
    #[cfg(windows)]
    {
        match socket_path() {
            Some(path) => claim_at(&path, files, may_hand_off),
            None => Claim::Alone,
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (files, may_hand_off);
        Claim::Alone
    }
}

/// The socket in `%LOCALAPPDATA%\CADCraft`.
#[cfg(windows)]
fn socket_path() -> Option<std::path::PathBuf> {
    let name = socket_name(std::env::var("SESSIONNAME").ok().as_deref());
    std::env::var_os("LOCALAPPDATA").filter(|v| !v.is_empty()).map(|d| std::path::PathBuf::from(d).join("CADCraft").join(name))
}

/// The socket's file name for a Windows session (`SESSIONNAME`: `Console`, `RDP-Tcp#3`…).
#[cfg_attr(not(windows), allow(dead_code))]
fn socket_name(session: Option<&str>) -> String {
    let session: String = session.unwrap_or_default().chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(32).collect();
    if session.is_empty() { "instance.sock".to_string() } else { format!("instance-{session}.sock") }
}

/// Held while a launch finds out whether it is the first, so launches that start together take turns.
#[cfg(windows)]
fn election(socket: &std::path::Path) -> Option<std::fs::File> {
    if let Some(dir) = socket.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(socket.with_extension("lock")).ok()?;
    let deadline = std::time::Instant::now() + TIMEOUT;
    loop {
        match file.try_lock() {
            Ok(()) => return Some(file),
            Err(std::fs::TryLockError::WouldBlock) if std::time::Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            Err(e) => {
                log::info!("single instance: no election lock ({e})");
                return None;
            }
        }
    }
}

/// [`claim`] with the socket at `path`.
#[cfg(windows)]
fn claim_at(path: &std::path::Path, files: &[String], may_hand_off: bool) -> Claim {
    use uds_windows::{UnixListener, UnixStream};
    let _election = election(path);
    for _ in 0..3 {
        match UnixStream::connect(path) {
            Ok(stream) => {
                if !may_hand_off {
                    return Claim::Alone;
                }
                return match hand_off(&stream, files) {
                    Ok(()) => Claim::HandedOff,
                    Err(e) => {
                        log::warn!("couldn't hand the files to the running CADCraft ({e}); opening a new window");
                        Claim::Alone
                    }
                };
            }
            Err(_) => match std::fs::remove_file(path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                    std::thread::sleep(Duration::from_millis(50));
                    continue;
                }
                Err(e) => {
                    log::info!("single instance off: can't clear {} ({e})", path.display());
                    return Claim::Alone;
                }
            },
        }
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match UnixListener::bind(path) {
            Ok(listener) => return serve(listener),
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => continue,
            Err(e) => {
                log::info!("single instance off: can't listen at {} ({e})", path.display());
                return Claim::Alone;
            }
        }
    }
    Claim::Alone
}

/// Send `files` over `stream` and wait for the running app's `ok`.
#[cfg(windows)]
fn hand_off(stream: &uds_windows::UnixStream, files: &[String]) -> std::io::Result<()> {
    use std::io::{BufRead, Read, Write};
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.set_write_timeout(Some(TIMEOUT))?;
    let mut writer = stream;
    writer.write_all(request(files).as_bytes())?;
    let mut reply = String::new();
    std::io::BufReader::new(stream.take(16)).read_line(&mut reply)?;
    if reply == "ok\n" { Ok(()) } else { Err(std::io::Error::other("the running CADCraft didn't confirm")) }
}

/// Accept launches on a background thread for as long as the process runs.
#[cfg(windows)]
fn serve(listener: uds_windows::UnixListener) -> Claim {
    let server = Server::new();
    let inbox = Server { inbox: Arc::clone(&server.inbox) };
    let started = std::thread::Builder::new().name("single-instance".into()).spawn(move || {
        for stream in listener.incoming() {
            let result = stream.and_then(|s| receive(&s, &inbox));
            if let Err(e) = result {
                log::warn!("single instance: a launch's request failed ({e})");
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    });
    match started {
        Ok(_) => Claim::Primary(server),
        Err(e) => {
            log::warn!("single instance off: no listener thread ({e})");
            Claim::Alone
        }
    }
}

/// Read one launch's request, queue its files and confirm.
#[cfg(windows)]
fn receive(stream: &uds_windows::UnixStream, server: &Server) -> std::io::Result<()> {
    use std::io::{BufRead, Read, Write};
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.set_write_timeout(Some(TIMEOUT))?;
    let mut line = String::new();
    std::io::BufReader::new(stream.take(MAX_REQUEST)).read_line(&mut line)?;
    if line.is_empty() {
        return Ok(());
    }
    let paths = parse_request(&line).ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "not an open request"))?;
    server.deliver(paths);
    let mut writer = stream;
    writer.write_all(b"ok\n")
}
