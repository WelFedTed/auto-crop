// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! A deliberately tiny HTTP/1.1 reader and writer for the labeller (std only, no HTTP crate; see
//! docs/policy/ci-guards.md). It answers one request per connection and closes it. It accepts
//! only what the labeller's page sends: `GET` and `POST` with an origin-form target, no chunked
//! bodies, no upgrades, bounded header and body sizes.

use std::collections::BTreeMap;
use std::io::{Read, Write};

pub const MAX_HEADER_BYTES: usize = 16 * 1024;
pub const MAX_BODY_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Default)]
pub struct Request {
    pub method: String,
    /// The path without the query; never percent-decoded (a `%` in it is refused).
    pub path: String,
    pub query: Vec<(String, String)>,
    /// Header names in lower case.
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }

    pub fn param(&self, name: &str) -> Option<&str> {
        self.query
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
    pub extra: Vec<(&'static str, String)>,
}

impl Response {
    pub fn new(status: u16, content_type: &'static str, body: Vec<u8>) -> Self {
        Self {
            status,
            content_type,
            body,
            extra: Vec::new(),
        }
    }

    pub fn text(status: u16, msg: &str) -> Self {
        Self::new(status, "text/plain; charset=utf-8", msg.as_bytes().to_vec())
    }

    pub fn json(status: u16, v: &serde_json::Value) -> Self {
        Self::new(
            status,
            "application/json",
            serde_json::to_vec(v).unwrap_or_default(),
        )
    }

    pub fn with(mut self, name: &'static str, value: String) -> Self {
        self.extra.push((name, value));
        self
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        415 => "Unsupported Media Type",
        422 => "Unprocessable Entity",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        _ => "Error",
    }
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' => {
                let hi = hex_val(*b.get(i + 1)?)?;
                let lo = hex_val(*b.get(i + 2)?)?;
                out.push(hi * 16 + lo);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

/// Parses a request head (everything before the blank line) without reading a body.
pub fn parse_head(head: &str) -> Result<Request, (u16, &'static str)> {
    let mut lines = head.split("\r\n");
    let first = lines.next().ok_or((400, "empty request"))?;
    let mut parts = first.split(' ');
    let (method, target, version) = (
        parts.next().unwrap_or_default(),
        parts.next().unwrap_or_default(),
        parts.next().unwrap_or_default(),
    );
    if parts.next().is_some() || !version.starts_with("HTTP/1.") {
        return Err((400, "malformed request line"));
    }
    if !target.starts_with('/') || target.starts_with("//") {
        return Err((400, "only origin-form request targets are accepted"));
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    if path.bytes().any(|b| !(0x21..0x7f).contains(&b))
        || path.contains(['%', '\\'])
        || path.split('/').any(|seg| seg == ".." || seg == ".")
    {
        return Err((400, "bad path"));
    }
    let mut params = Vec::new();
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let (Some(k), Some(v)) = (percent_decode(k), percent_decode(v)) else {
            return Err((400, "bad query string"));
        };
        params.push((k, v));
    }
    let mut headers = BTreeMap::new();
    for line in lines.filter(|l| !l.is_empty()) {
        let (name, value) = line.split_once(':').ok_or((400, "malformed header"))?;
        let name = name.trim().to_ascii_lowercase();
        if headers.insert(name, value.trim().to_owned()).is_some() {
            return Err((400, "duplicate header"));
        }
    }
    Ok(Request {
        method: method.to_owned(),
        path: path.to_owned(),
        query: params,
        headers,
        body: Vec::new(),
    })
}

/// Reads one request from `stream`: the head (bounded), then `Content-Length` bytes (bounded).
pub fn read_request(stream: &mut impl Read) -> Result<Request, (u16, &'static str)> {
    let mut buf = Vec::with_capacity(2048);
    let mut chunk = [0u8; 2048];
    let head_end = loop {
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break p;
        }
        if buf.len() > MAX_HEADER_BYTES {
            return Err((431, "request head too large"));
        }
        let n = stream.read(&mut chunk).map_err(|_| (400, "read error"))?;
        if n == 0 {
            return Err((400, "connection closed"));
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    if head_end > MAX_HEADER_BYTES {
        return Err((431, "request head too large"));
    }
    let head = std::str::from_utf8(&buf[..head_end]).map_err(|_| (400, "head is not UTF-8"))?;
    let mut req = parse_head(head)?;
    if req.header("transfer-encoding").is_some() {
        return Err((400, "chunked bodies are not accepted"));
    }
    let len: usize = match req.header("content-length") {
        None => 0,
        Some(v) => v.parse().map_err(|_| (400, "bad content-length"))?,
    };
    if len > MAX_BODY_BYTES {
        return Err((413, "body too large"));
    }
    let mut body = buf[head_end + 4..].to_vec();
    if body.len() > len {
        body.truncate(len);
    }
    while body.len() < len {
        let n = stream.read(&mut chunk).map_err(|_| (400, "read error"))?;
        if n == 0 {
            return Err((400, "body cut short"));
        }
        body.extend_from_slice(&chunk[..n.min(len - body.len())]);
    }
    req.body = body;
    Ok(req)
}

/// Writes the response with the security headers every reply carries.
pub fn write_response(stream: &mut impl Write, r: &Response) -> std::io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\n\
         Cross-Origin-Resource-Policy: same-origin\r\nCross-Origin-Opener-Policy: same-origin\r\n\
         X-Frame-Options: DENY\r\nConnection: close\r\n",
        r.status,
        reason(r.status),
        r.content_type,
        r.body.len()
    );
    for (k, v) in &r.extra {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    stream.write_all(&r.body)?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head(s: &str) -> Result<Request, (u16, &'static str)> {
        parse_head(s)
    }

    #[test]
    fn a_normal_request_parses() {
        let r =
            head("GET /api/list?t=ab%20c&x=1 HTTP/1.1\r\nHost: 127.0.0.1:5\r\nOrigin: http://a")
                .expect("ok");
        assert_eq!((r.method.as_str(), r.path.as_str()), ("GET", "/api/list"));
        assert_eq!(r.param("t"), Some("ab c"));
        assert_eq!(r.header("host"), Some("127.0.0.1:5"));
    }

    #[test]
    fn hostile_requests_are_refused() {
        for bad in [
            "GET /../etc/passwd HTTP/1.1",
            "GET /a/../b HTTP/1.1",
            "GET /%2e%2e/x HTTP/1.1",
            "GET /a\\b HTTP/1.1",
            "GET //evil.example/x HTTP/1.1",
            "GET http://evil.example/ HTTP/1.1",
            "GET x HTTP/1.1",
            "GET / HTTP/2",
            "GET /a b HTTP/1.1",
            "GET /?t=%zz HTTP/1.1",
        ] {
            assert!(head(bad).is_err(), "{bad} should be refused");
        }
        assert!(head("GET / HTTP/1.1\r\nHost: a\r\nHost: b").is_err());
        assert!(head("GET / HTTP/1.1\r\nno colon here").is_err());
    }

    #[test]
    fn bodies_and_sizes_are_bounded() {
        let ok = b"POST /x HTTP/1.1\r\nContent-Length: 3\r\n\r\nabcEXTRA".to_vec();
        let r = read_request(&mut ok.as_slice()).expect("ok");
        assert_eq!(r.body, b"abc");
        let big = format!(
            "POST /x HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            MAX_BODY_BYTES + 1
        );
        assert_eq!(read_request(&mut big.as_bytes()).expect_err("big").0, 413);
        let chunked = b"POST /x HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
        assert_eq!(
            read_request(&mut chunked.as_slice())
                .expect_err("chunked")
                .0,
            400
        );
        let short = b"POST /x HTTP/1.1\r\nContent-Length: 10\r\n\r\nabc".to_vec();
        assert!(read_request(&mut short.as_slice()).is_err());
        let huge = vec![b'a'; MAX_HEADER_BYTES + 100];
        assert_eq!(read_request(&mut huge.as_slice()).expect_err("huge").0, 431);
    }

    #[test]
    fn every_response_carries_the_security_headers() {
        let mut out = Vec::new();
        write_response(&mut out, &Response::text(403, "no")).expect("writes");
        let text = String::from_utf8(out).expect("utf8");
        for h in [
            "Cache-Control: no-store",
            "X-Content-Type-Options: nosniff",
            "Referrer-Policy: no-referrer",
            "Connection: close",
        ] {
            assert!(text.contains(h), "{h} missing in {text}");
        }
        assert!(text.starts_with("HTTP/1.1 403 Forbidden"));
    }
}
