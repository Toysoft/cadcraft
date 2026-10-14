//! Loopback JSON-lines control server: one request per line, one reply per line.
//! This is the transport the MCP server (`cadcraft-cli mcp --connect`) wraps.
//!
//! Lines are bounded ([`MAX_LINE`]): a longer one is refused and the connection closed, without
//! buffering the rest. So is the number of connections served at once ([`MAX_CONNECTIONS`]).

use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use cadcraft_ui_egui::ControlRequest;
use serde_json::{Value, json};

/// Longest accepted request line (bytes, without the newline). Generous, so requests that carry
/// whole drawings or images as base64 still fit.
const MAX_LINE: usize = 256 * 1024 * 1024;
/// Connections served at once; further ones get an error line and are closed.
const MAX_CONNECTIONS: usize = 16;
/// How long a connection may wait for the rest of a request (or the next one) before it is closed,
/// and how long a reply write may block. Frees the slot of an abandoned or stalled client.
const IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60);

pub fn start(port: u16, ctx: egui::Context) -> Receiver<ControlRequest> {
    let (tx, rx) = channel::<ControlRequest>();
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(e) => {
            log::error!("control server failed to bind 127.0.0.1:{port}: {e}");
            return rx;
        }
    };
    log::info!("control server listening on 127.0.0.1:{port}");
    std::thread::spawn(move || accept_loop(listener, tx, ctx, IDLE_TIMEOUT));
    rx
}

/// A connection's place among [`MAX_CONNECTIONS`], given back when the connection ends (also when
/// its thread can't be started, or panics).
struct Slot(Arc<AtomicUsize>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

fn accept_loop(listener: TcpListener, tx: Sender<ControlRequest>, ctx: egui::Context, idle: Duration) {
    let open = Arc::new(AtomicUsize::new(0));
    for mut stream in listener.incoming().flatten() {
        if open.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
            open.fetch_sub(1, Ordering::SeqCst);
            let _ = writeln!(stream, "{}", json!({"ok": false, "error": "too many control connections"}));
            continue;
        }
        let slot = Slot(Arc::clone(&open));
        let tx = tx.clone();
        let ctx = ctx.clone();
        let _ = std::thread::Builder::new().spawn(move || {
            let _slot = slot;
            serve(stream, tx, ctx, idle);
        });
    }
}

/// Read one `\n`-terminated line of at most `max` bytes into `buf` (newline stripped).
/// `Ok(false)` at end of stream; `Err` when the line is too long or the read fails.
fn read_bounded_line(reader: &mut impl BufRead, buf: &mut Vec<u8>, max: usize) -> Result<bool, String> {
    buf.clear();
    // `take` bounds the bytes buffered for one line: a peer can't make us allocate more than `max`.
    let limit = u64::try_from(max).unwrap_or(u64::MAX).saturating_add(1);
    let n = reader.take(limit).read_until(b'\n', buf).map_err(read_error)?;
    if n == 0 {
        return Ok(false);
    }
    if buf.last() == Some(&b'\n') {
        buf.pop();
        if buf.last() == Some(&b'\r') {
            buf.pop();
        }
        return Ok(true);
    }
    if buf.len() > max {
        return Err(format!("request line longer than {max} bytes; closing the connection"));
    }
    // Last line without a newline, then end of stream.
    Ok(true)
}

/// A failed read as the error line sent before closing. The socket's read timeout running out
/// (`WouldBlock` on Unix, `TimedOut` on Windows) means the client was idle.
fn read_error(e: std::io::Error) -> String {
    match e.kind() {
        ErrorKind::WouldBlock | ErrorKind::TimedOut => "connection idle too long; closing it".to_string(),
        _ => e.to_string(),
    }
}

fn serve(stream: TcpStream, tx: Sender<ControlRequest>, ctx: egui::Context, idle: Duration) {
    // The clone below shares the socket, so these deadlines cover reads and writes alike.
    if stream.set_read_timeout(Some(idle)).is_err() || stream.set_write_timeout(Some(idle)).is_err() {
        return;
    }
    let Ok(read) = stream.try_clone() else { return };
    let mut out = stream;
    let mut reader = BufReader::new(read);
    let mut buf = Vec::new();
    loop {
        match read_bounded_line(&mut reader, &mut buf, MAX_LINE) {
            Ok(true) => {}
            Ok(false) => break,
            Err(e) => {
                let _ = writeln!(out, "{}", json!({"ok": false, "error": e}));
                break;
            }
        }
        // Bytes that aren't UTF-8 end the connection, as `BufRead::lines` did.
        let Ok(line) = std::str::from_utf8(&buf) else { break };
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(line) {
            Ok(msg) => {
                let id = msg.get("id").cloned().unwrap_or(Value::Null);
                let method = msg.get("method").and_then(Value::as_str).unwrap_or("").to_string();
                let params = msg.get("params").cloned().unwrap_or(json!({}));
                let (req, pending) = ControlRequest::new(method, params);
                if tx.send(req).is_err() {
                    break;
                }
                ctx.request_repaint();
                // On timeout the request is cancelled if the app hasn't started it, and the reply's
                // `state` says whether it can still have run.
                let mut r = pending.wait(Duration::from_secs(60));
                if let Some(o) = r.as_object_mut() {
                    o.insert("id".into(), id);
                }
                r
            }
            Err(e) => json!({"ok": false, "error": format!("bad JSON: {e}")}),
        };
        if writeln!(out, "{reply}").is_err() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_are_read_up_to_the_limit() {
        let mut buf = Vec::new();
        let mut input: &[u8] = b"aaaaaaaaaa\nbb\r\ncc";
        assert_eq!(read_bounded_line(&mut input, &mut buf, 10), Ok(true));
        assert_eq!(buf, b"aaaaaaaaaa", "a line exactly at the limit is accepted");
        assert_eq!(read_bounded_line(&mut input, &mut buf, 10), Ok(true));
        assert_eq!(buf, b"bb", "CRLF is stripped");
        assert_eq!(read_bounded_line(&mut input, &mut buf, 10), Ok(true));
        assert_eq!(buf, b"cc", "a last line without a newline is still read");
        assert_eq!(read_bounded_line(&mut input, &mut buf, 10), Ok(false), "then end of stream");
    }

    #[test]
    fn an_overlong_line_is_refused_without_reading_the_rest() {
        let mut buf = Vec::new();
        let long = [b'x'; 100];
        let mut input: &[u8] = &long;
        assert!(read_bounded_line(&mut input, &mut buf, 10).is_err());
        assert!(buf.len() <= 11, "at most limit + 1 bytes are buffered, got {}", buf.len());
        assert_eq!(input.len(), 100 - 11, "the rest of the line is left unread");
    }

    /// The control server on a loopback port, with a stand-in UI that echoes each method.
    fn serve_echo() -> std::net::SocketAddr {
        serve_echo_with(IDLE_TIMEOUT)
    }

    fn serve_echo_with(idle: Duration) -> std::net::SocketAddr {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = channel::<ControlRequest>();
        std::thread::spawn(move || accept_loop(listener, tx, egui::Context::default(), idle));
        std::thread::spawn(move || {
            for req in rx {
                let _ = req.reply.send(json!({"ok": true, "method": req.method}));
            }
        });
        addr
    }

    #[test]
    fn requests_are_answered_with_their_ids() {
        let addr = serve_echo();
        let mut c = TcpStream::connect(addr).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        c.write_all(b"{\"id\":3,\"method\":\"ping\"}\r\n\n{\"id\":4,\"method\":\"pong\"}\n").unwrap();
        let mut r = BufReader::new(c);
        for (id, method) in [(3, "ping"), (4, "pong")] {
            let mut line = String::new();
            r.read_line(&mut line).unwrap();
            let v: Value = serde_json::from_str(&line).unwrap();
            assert_eq!((v["id"].as_i64(), v["method"].as_str()), (Some(id), Some(method)), "{line}");
        }
    }

    #[test]
    fn extra_connections_are_refused() {
        let addr = serve_echo();
        // Idle connections are accepted in order and held open.
        let held: Vec<TcpStream> = (0..MAX_CONNECTIONS).map(|_| TcpStream::connect(addr).unwrap()).collect();
        let mut extra = TcpStream::connect(addr).unwrap();
        extra.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut line = String::new();
        BufReader::new(&mut extra).read_line(&mut line).unwrap();
        assert!(line.contains("too many control connections"), "{line}");
        drop(held);
    }

    #[test]
    fn idle_connections_release_their_slot() {
        let addr = serve_echo_with(Duration::from_millis(200));
        // Fill every slot with clients that never send a byte.
        let held: Vec<TcpStream> = (0..MAX_CONNECTIONS).map(|_| TcpStream::connect(addr).unwrap()).collect();
        std::thread::sleep(Duration::from_millis(1000));
        let mut c = TcpStream::connect(addr).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        c.write_all(b"{\"id\":1,\"method\":\"ping\"}\n").unwrap();
        let mut line = String::new();
        BufReader::new(&mut c).read_line(&mut line).unwrap();
        assert!(line.contains("\"ping\""), "a new client is served once the idle ones time out: {line}");
        let mut notice = String::new();
        BufReader::new(&held[0]).read_line(&mut notice).unwrap();
        assert!(notice.contains("idle too long"), "an idle client is told why it was closed: {notice}");
        drop(held);
    }
}
