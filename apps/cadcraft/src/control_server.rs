//! Loopback JSON-lines control server: one request per line, one reply per line.
//! This is the transport the MCP server (`cadcraft-cli mcp --connect`) wraps.
//!
//! Lines are bounded ([`MAX_LINE`]): a longer one is refused and the connection closed, without
//! buffering the rest. So is the number of connections served at once ([`MAX_CONNECTIONS`]).
//!
//! Every request carries the control token (`cadcraft_engine::control_token`): a request without
//! the right one is refused and the connection closed, and so is a line that isn't JSON. Both
//! keep a web page from driving the app: a browser will send a `fetch` body to the loopback
//! port, but as an HTTP request, whose first line is not JSON, and without the token in the
//! user's settings directory.

use std::io::{BufRead, BufReader, Read, Write};
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

pub fn start(port: u16, ctx: egui::Context, token: Arc<str>) -> Receiver<ControlRequest> {
    let (tx, rx) = channel::<ControlRequest>();
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(e) => {
            log::error!("control server failed to bind 127.0.0.1:{port}: {e}");
            return rx;
        }
    };
    log::info!("control server listening on 127.0.0.1:{port}");
    std::thread::spawn(move || accept_loop(listener, tx, ctx, token));
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

fn accept_loop(listener: TcpListener, tx: Sender<ControlRequest>, ctx: egui::Context, token: Arc<str>) {
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
        let token = Arc::clone(&token);
        let _ = std::thread::Builder::new().spawn(move || {
            let _slot = slot;
            serve(stream, tx, ctx, &token);
        });
    }
}

/// Read one `\n`-terminated line of at most `max` bytes into `buf` (newline stripped).
/// `Ok(false)` at end of stream; `Err` when the line is too long or the read fails.
fn read_bounded_line(reader: &mut impl BufRead, buf: &mut Vec<u8>, max: usize) -> Result<bool, String> {
    buf.clear();
    // `take` bounds the bytes buffered for one line: a peer can't make us allocate more than `max`.
    let limit = u64::try_from(max).unwrap_or(u64::MAX).saturating_add(1);
    let n = reader.take(limit).read_until(b'\n', buf).map_err(|e| e.to_string())?;
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

/// What a request's `token` field must say: the first field checked, before anything is run.
const TOKEN_ERROR: &str = "missing or wrong `token`: send the contents of <settings>/control.token with every request (docs/control-protocol.md)";

fn serve(stream: TcpStream, tx: Sender<ControlRequest>, ctx: egui::Context, token: &str) {
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
                let presented = msg.get("token").and_then(Value::as_str).unwrap_or("");
                if !cadcraft_engine::control_token::matches(token, presented) {
                    let _ = writeln!(out, "{}", json!({"id": id, "ok": false, "error": TOKEN_ERROR}));
                    break;
                }
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
            // Not JSON: say so and hang up. Reading on would let an HTTP request's body in behind
            // its header lines.
            Err(e) => {
                let _ = writeln!(out, "{}", json!({"ok": false, "error": format!("bad JSON: {e}; closing the connection")}));
                break;
            }
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

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    /// The control server on a loopback port, with a stand-in UI that echoes each method.
    fn serve_echo() -> std::net::SocketAddr {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = channel::<ControlRequest>();
        std::thread::spawn(move || accept_loop(listener, tx, egui::Context::default(), Arc::from(TOKEN)));
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
        c.write_all(
            format!("{{\"id\":3,\"token\":\"{TOKEN}\",\"method\":\"ping\"}}\r\n\n{{\"id\":4,\"token\":\"{TOKEN}\",\"method\":\"pong\"}}\n")
                .as_bytes(),
        )
        .unwrap();
        let mut r = BufReader::new(c);
        for (id, method) in [(3, "ping"), (4, "pong")] {
            let mut line = String::new();
            r.read_line(&mut line).unwrap();
            let v: Value = serde_json::from_str(&line).unwrap();
            assert_eq!((v["id"].as_i64(), v["method"].as_str()), (Some(id), Some(method)), "{line}");
        }
    }

    /// One request on a fresh connection; the reply line, then whether the server hung up.
    fn exchange(addr: std::net::SocketAddr, request: &str) -> (Value, bool) {
        let mut c = TcpStream::connect(addr).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        c.write_all(request.as_bytes()).unwrap();
        let mut r = BufReader::new(c);
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        let reply: Value = serde_json::from_str(&line).unwrap_or_else(|_| panic!("not JSON: {line:?}"));
        let mut rest = String::new();
        let closed = r.read_line(&mut rest).map(|n| n == 0).unwrap_or(false);
        (reply, closed)
    }

    #[test]
    fn requests_without_the_token_are_refused_and_the_connection_closed() {
        let addr = serve_echo();
        for bad in [
            "{\"id\":1,\"method\":\"ping\"}\n",
            "{\"id\":1,\"token\":\"\",\"method\":\"ping\"}\n",
            "{\"id\":1,\"token\":\"0123456789abcdef0123456789abcde\",\"method\":\"ping\"}\n",
            "{\"id\":1,\"token\":7,\"method\":\"ping\"}\n",
        ] {
            let (reply, closed) = exchange(addr, &format!("{bad}{{\"id\":2,\"token\":\"{TOKEN}\",\"method\":\"ping\"}}\n"));
            assert_eq!(reply["ok"], json!(false), "{bad:?}: {reply}");
            assert_eq!(reply["id"], json!(1), "{bad:?}: the refusal names the request");
            assert!(reply["error"].as_str().unwrap().contains("token"), "{bad:?}: {reply}");
            assert!(closed, "{bad:?}: the good request behind it must not be read");
        }
    }

    #[test]
    fn a_line_that_is_not_json_closes_the_connection() {
        // What a browser's `fetch("http://127.0.0.1:PORT", {method: "POST", body})` sends: the
        // request line and headers, a blank line, then the body. Nothing of it may run.
        let addr = serve_echo();
        let body = format!("{{\"id\":9,\"token\":\"{TOKEN}\",\"method\":\"ping\"}}");
        let http = format!("POST / HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{body}\n", body.len());
        let (reply, closed) = exchange(addr, &http);
        assert_eq!(reply["ok"], json!(false));
        assert!(reply["error"].as_str().unwrap().starts_with("bad JSON"), "{reply}");
        assert!(closed, "the connection ends at the first line that is not JSON");
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
}
