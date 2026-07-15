//! stdlib.md § system.net.Http — a minimal HTTP/1.1 client backing
//! `crate::native`'s `system.net.Http.get`/`post`. `http://` talks over a
//! plain `std::net::TcpStream`; `https://` wraps the same stream in a
//! `rustls::ClientConnection` seeded with Mozilla's bundled root store
//! (`webpki-roots`), so certificate validation (chain of trust, expiry,
//! hostname) actually happens by default — stdlib.md's TLS section is an
//! explicit MUST with "no option to disable validation", so any handshake
//! or certificate failure is left to surface as a plain I/O error from
//! `read`/`write` on the TLS stream, which `http_request` turns into the
//! same `IOException` as a connection failure would (no special-casing
//! needed: rustls already refuses to complete the handshake on a bad
//! certificate, so the first `write_all`/`read_to_end` on an invalid
//! connection simply errors).
//!
//! Every request sends `Connection: close` and reads to EOF rather than
//! tracking `Content-Length` for framing — simpler, and correct as long as
//! the server actually closes the connection when done (which `Connection:
//! close` asks for). `Transfer-Encoding: chunked` bodies are still
//! unwrapped (`dechunk`) since some servers chunk regardless of the
//! connection header.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::rc::Rc;
use std::sync::Arc;

use crate::error::VmError;
use crate::native::throw_native;
use crate::value::{Object, Value};

struct ParsedUrl {
    https: bool,
    host: String,
    port: u16,
    path: String,
}

fn throw_io(message: impl Into<String>) -> VmError {
    throw_native("IOException", message)
}

/// No IPv6-literal (`[::1]`) or userinfo/query-fragment handling — not
/// exercised by anything this client needs to talk to (its own test
/// server, or a plain `http(s)://host[:port]/path`).
fn parse_url(url: &str) -> Result<ParsedUrl, VmError> {
    let (scheme, rest) = url.split_once("://").ok_or_else(|| throw_io(format!("invalid URL: {url}")))?;
    let https = match scheme {
        "http" => false,
        "https" => true,
        _ => return Err(throw_io(format!("unsupported scheme: {scheme}"))),
    };
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], rest[i..].to_string()),
        None => (rest, "/".to_string()),
    };
    let default_port = if https { 443 } else { 80 };
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if !h.is_empty() && !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) => {
            (h.to_string(), p.parse().unwrap_or(default_port))
        }
        _ => (authority.to_string(), default_port),
    };
    if host.is_empty() {
        return Err(throw_io(format!("invalid URL: {url}")));
    }
    Ok(ParsedUrl { https, host, port, path: if path.is_empty() { "/".to_string() } else { path } })
}

fn build_request(parsed: &ParsedUrl, method: &str, body: Option<&str>) -> String {
    let mut req = format!("{method} {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n", parsed.path, parsed.host);
    if let Some(b) = body {
        req.push_str(&format!("Content-Length: {}\r\n", b.len()));
    }
    req.push_str("\r\n");
    if let Some(b) = body {
        req.push_str(b);
    }
    req
}

pub fn http_request(url: &str, method: &str, body: Option<&str>) -> Result<Value, VmError> {
    let parsed = parse_url(url)?;
    let request = build_request(&parsed, method, body);
    let tcp = TcpStream::connect((parsed.host.as_str(), parsed.port))
        .map_err(|e| throw_io(format!("connect {}:{}: {e}", parsed.host, parsed.port)))?;
    let raw = if parsed.https {
        https_roundtrip(tcp, &parsed.host, &request)?
    } else {
        plain_roundtrip(tcp, &request)?
    };
    parse_response(&raw)
}

fn plain_roundtrip(mut tcp: TcpStream, request: &str) -> Result<Vec<u8>, VmError> {
    tcp.write_all(request.as_bytes()).map_err(|e| throw_io(e.to_string()))?;
    let mut raw = Vec::new();
    tcp.read_to_end(&mut raw).map_err(|e| throw_io(e.to_string()))?;
    Ok(raw)
}

fn https_roundtrip(tcp: TcpStream, host: &str, request: &str) -> Result<Vec<u8>, VmError> {
    let mut root_store = rustls::RootCertStore::empty();
    root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let config = rustls::ClientConfig::builder().with_root_certificates(root_store).with_no_client_auth();
    let server_name = rustls::pki_types::ServerName::try_from(host.to_string())
        .map_err(|_| throw_io(format!("invalid hostname: {host}")))?;
    let conn = rustls::ClientConnection::new(Arc::new(config), server_name)
        .map_err(|e| throw_io(format!("TLS setup failed: {e}")))?;
    let mut tls = rustls::StreamOwned::new(conn, tcp);
    tls.write_all(request.as_bytes()).map_err(|e| throw_io(format!("TLS: {e}")))?;
    let mut raw = Vec::new();
    match tls.read_to_end(&mut raw) {
        Ok(_) => {}
        // A server that closes without a clean TLS `close_notify` still
        // delivered a complete response by then; treated like EOF on a
        // plain socket rather than a hard failure.
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof && !raw.is_empty() => {}
        Err(e) => return Err(throw_io(format!("TLS: {e}"))),
    }
    Ok(raw)
}

fn find_double_crlf(raw: &[u8]) -> Option<usize> {
    raw.windows(4).position(|w| w == b"\r\n\r\n")
}

fn parse_response(raw: &[u8]) -> Result<Value, VmError> {
    let header_end = find_double_crlf(raw).ok_or_else(|| throw_io("malformed HTTP response: no header terminator"))?;
    let header_text = String::from_utf8_lossy(&raw[..header_end]);
    let mut lines = header_text.split("\r\n");
    let status_line = lines.next().unwrap_or("");
    let status_code = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<i64>().ok())
        .ok_or_else(|| throw_io(format!("malformed HTTP status line: {status_line}")))?;

    let mut headers = Vec::new();
    let mut chunked = false;
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.trim().eq_ignore_ascii_case("transfer-encoding") && value.to_ascii_lowercase().contains("chunked") {
                chunked = true;
            }
        }
        headers.push(line.to_string());
    }

    let body_bytes = &raw[header_end + 4..];
    let body = if chunked { dechunk(body_bytes) } else { body_bytes.to_vec() };

    let mut fields = HashMap::new();
    fields.insert("statusCode".to_string(), Value::Int(status_code));
    fields.insert("body".to_string(), Value::Str(Rc::new(String::from_utf8_lossy(&body).into_owned())));
    let header_values: Vec<Value> = headers.into_iter().map(|h| Value::Str(Rc::new(h))).collect();
    fields.insert("headers".to_string(), Value::Array(Rc::new(RefCell::new(header_values))));
    Ok(Value::Object(Rc::new(RefCell::new(Object { class_name: "system.net.HttpResponse".to_string(), fields }))))
}

/// Unwraps `Transfer-Encoding: chunked` (RFC 7230 § 4.1): `<size in
/// hex>\r\n<size bytes>\r\n`, repeated, terminated by a zero-size chunk.
/// Chunk extensions (`;name=value` after the size) are accepted and
/// ignored; trailer headers after the terminating chunk are not read
/// (already-read `raw` — from an EOF-terminated `Connection: close`
/// response — has nothing meaningful past them for this client).
fn dechunk(mut data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let Some(line_end) = data.windows(2).position(|w| w == b"\r\n") else {
            break;
        };
        let size_str = String::from_utf8_lossy(&data[..line_end]);
        let size_str = size_str.split(';').next().unwrap_or("").trim();
        let Ok(size) = usize::from_str_radix(size_str, 16) else {
            break;
        };
        data = &data[line_end + 2..];
        if size == 0 {
            break;
        }
        if data.len() < size {
            out.extend_from_slice(data);
            break;
        }
        out.extend_from_slice(&data[..size]);
        data = &data[size..];
        if data.len() >= 2 && &data[..2] == b"\r\n" {
            data = &data[2..];
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufRead;
    use std::net::TcpListener;

    /// `Http.get`/`post` need a real second party to talk to, and the VM
    /// itself is single-threaded (no `system.thread` support at all, let
    /// alone one usable from Rust test setup) — so unlike every other
    /// stdlib test in this project (plain YAML fixtures under `tests/`),
    /// this one drives `http_request` directly from Rust and spins up the
    /// "server" side as a real OS thread, which runs concurrently with the
    /// client call by construction.
    fn serve_once(response: &'static str) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" || line.is_empty() {
                    break;
                }
            }
            stream.write_all(response.as_bytes()).unwrap();
        });
        port
    }

    // `Value` has no `PartialEq` (never needed one before this test), so
    // fields are unpacked by hand rather than via `assert_eq!` on `Value`.
    fn field_int(obj: &Object, name: &str) -> i64 {
        match obj.fields.get(name) {
            Some(Value::Int(n)) => *n,
            other => panic!("expected int field {name:?}, got {other:?}"),
        }
    }

    fn field_str(obj: &Object, name: &str) -> String {
        match obj.fields.get(name) {
            Some(Value::Str(s)) => (**s).clone(),
            other => panic!("expected string field {name:?}, got {other:?}"),
        }
    }

    #[test]
    fn get_plain_http() {
        let port = serve_once("HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\nhello world");
        let result = http_request(&format!("http://127.0.0.1:{port}/"), "GET", None).unwrap();
        let Value::Object(obj) = result else { panic!("expected object") };
        let obj = obj.borrow();
        assert_eq!(field_int(&obj, "statusCode"), 200);
        assert_eq!(field_str(&obj, "body"), "hello world");
    }

    #[test]
    fn post_with_body() {
        let port = serve_once("HTTP/1.1 201 Created\r\n\r\ncreated");
        let result = http_request(&format!("http://127.0.0.1:{port}/items"), "POST", Some("payload")).unwrap();
        let Value::Object(obj) = result else { panic!("expected object") };
        let obj = obj.borrow();
        assert_eq!(field_int(&obj, "statusCode"), 201);
        assert_eq!(field_str(&obj, "body"), "created");
    }

    #[test]
    fn chunked_response_is_decoded() {
        let port = serve_once("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n1\r\n \r\n5\r\nworld\r\n0\r\n\r\n");
        let result = http_request(&format!("http://127.0.0.1:{port}/"), "GET", None).unwrap();
        let Value::Object(obj) = result else { panic!("expected object") };
        let obj = obj.borrow();
        assert_eq!(field_str(&obj, "body"), "hello world");
    }

    /// Real internet access, so `#[ignore]`d by default (not run by plain
    /// `cargo test`, only `cargo test -- --ignored`) — this is the one
    /// test that actually exercises `https_roundtrip`'s TLS handshake and
    /// certificate validation against a real server, since nothing else in
    /// this suite can stand in for a trusted CA-signed certificate.
    #[test]
    #[ignore = "requires internet access"]
    fn get_https_real_server() {
        let result = http_request("https://example.com/", "GET", None).unwrap();
        let Value::Object(obj) = result else { panic!("expected object") };
        let obj = obj.borrow();
        assert_eq!(field_int(&obj, "statusCode"), 200);
        assert!(field_str(&obj, "body").contains("Example Domain"));
    }

    #[test]
    fn connection_refused_throws_io_exception() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener); // frees the port without anything listening on it
        let err = http_request(&format!("http://127.0.0.1:{port}/"), "GET", None).unwrap_err();
        match err {
            VmError::Thrown(Value::Object(obj)) => assert_eq!(obj.borrow().class_name, "IOException"),
            other => panic!("expected IOException, got {other:?}"),
        }
    }
}
