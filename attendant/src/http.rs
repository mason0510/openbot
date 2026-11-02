use std::io::{ErrorKind, Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::auth::{self, normalize_email, AuthReply};
use crate::store::Store;

/** Handles one HTTP/1.1 request. The connection is closed after the response.
 * Guarantees the router sees the full Content-Length body when those bytes arrive within 10 seconds and the body is at most 64 KiB.
 * Does not support chunked transfer or a second request on the same connection.
 */
pub fn handle(mut stream: TcpStream, store: &Arc<Mutex<Store>>, from: &str, now: u64) -> Result<(), String> {
    stream.set_read_timeout(Some(Duration::from_secs(10))).ok();
    stream.set_write_timeout(Some(Duration::from_secs(20))).ok();
    let buffer = read_request(&mut stream)?;
    let text = String::from_utf8_lossy(&buffer);
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((text.as_ref(), ""));
    let mut lines = head.lines();
    let request = lines.next().unwrap_or("");
    let mut parts = request.split_whitespace();
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("");
    let mut content_length = 0_usize;
    let mut authorization = String::new();
    for line in lines {
        let Some((name, value)) = line.split_once(':') else { continue };
        if name.eq_ignore_ascii_case("content-length") {
            content_length = value.trim().parse().unwrap_or(0);
        } else if name.eq_ignore_ascii_case("authorization") {
            authorization = value.trim().to_string();
        }
    }
    let body = if body.len() >= content_length { &body[..content_length] } else { body };
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let (status, payload) = route(method, path, query, body, &authorization, store, from, now);
    let response = if status == 204 {
        "HTTP/1.1 204\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_string()
    } else {
        format!(
            "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{payload}",
            payload.len()
        )
    };
    stream.write_all(response.as_bytes()).map_err(|error| error.to_string())
}

/** Reads headers and the declared body. A single `read` is not enough: TLS often delivers the body in a later packet. */
fn read_request(stream: &mut TcpStream) -> Result<Vec<u8>, String> {
    let mut buffer = Vec::with_capacity(1024);
    let mut tmp = [0_u8; 4096];
    loop {
        if let Some(header_end) = find_header_end(&buffer) {
            let content_length = content_length_of(&buffer[..header_end]);
            let body_have = buffer.len().saturating_sub(header_end + 4);
            if body_have >= content_length || buffer.len() > 65_536 {
                let end = header_end + 4 + content_length.min(body_have);
                buffer.truncate(end);
                return Ok(buffer);
            }
        } else if buffer.len() > 65_536 {
            return Ok(buffer);
        }
        match stream.read(&mut tmp) {
            Ok(0) => return Ok(buffer),
            Ok(count) => buffer.extend_from_slice(&tmp[..count]),
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == ErrorKind::WouldBlock || error.kind() == ErrorKind::TimedOut => {
                return Ok(buffer);
            }
            Err(error) => return Err(error.to_string()),
        }
    }
}

/** Returns the byte index of the blank line between headers and body. */
fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

/** Reads Content-Length from the header block. A missing or bad value means no body. */
fn content_length_of(headers: &[u8]) -> usize {
    let text = String::from_utf8_lossy(headers);
    for line in text.lines() {
        let Some((name, value)) = line.split_once(':') else { continue };
        if name.eq_ignore_ascii_case("content-length") {
            return value.trim().parse().unwrap_or(0);
        }
    }
    0
}

/** Routes the account calls. Unknown paths return 404. */
fn route(
    method: &str,
    path: &str,
    query: &str,
    body: &str,
    authorization: &str,
    store: &Arc<Mutex<Store>>,
    from: &str,
    now: u64,
) -> (u16, String) {
    match (method, path) {
        ("GET", "/health/live") => (200, r#"{"status":"ok"}"#.to_string()),
        ("POST", "/v1/auth/email/start") => start(body, store, from, now),
        ("POST", "/v1/auth/email/verify") => verify(body, store, now),
        // The phone sends device fields too. Verification still uses challengeId and code.
        ("POST", "/v1/mobile-auth/email/verify") => verify(body, store, now),
        ("POST", "/v1/mobile-auth/ticket") => issue_ticket(body, authorization, store, now),
        ("DELETE", "/v1/mobile-auth/ticket") => cancel_ticket(query, authorization, store, now),
        ("POST", "/v1/mobile-auth/redeem") => redeem_ticket(body, store, now),
        ("GET", "/v1/mobile-auth/devices") => list_devices(query, authorization, store, now),
        ("POST", "/v2/remote/hosts/register") => register_host(body, authorization, store, now),
        ("GET", "/v2/remote/hosts/") | ("GET", "/v2/remote/hosts") => list_hosts(authorization, store, now),
        ("GET", "/v1/me") => me(authorization, store, now),
        _ => (404, r#"{"error":"not_found"}"#.to_string()),
    }
}

/** Parses the start body and runs the sign-in under the store lock. */
fn start(body: &str, store: &Arc<Mutex<Store>>, from: &str, now: u64) -> (u16, String) {
    let email = match json_string(body, "email").and_then(|value| normalize_email(&value)) {
        Ok(email) => email,
        Err(_) => return (400, r#"{"error":"invalid_email"}"#.to_string()),
    };
    let mut guard = match store.lock() {
        Ok(guard) => guard,
        Err(_) => return (500, r#"{"error":"mail_failed"}"#.to_string()),
    };
    match auth::start(&mut guard, &email, from, now) {
        AuthReply::Json(status, payload) => (status, payload),
    }
}

/** Parses the verify body. The desktop client sends `challengeId` and `code`, not the email. */
fn verify(body: &str, store: &Arc<Mutex<Store>>, now: u64) -> (u16, String) {
    let challenge_id = match json_string(body, "challengeId") {
        Ok(value) => value,
        Err(_) => return (400, r#"{"error":"invalid_challenge"}"#.to_string()),
    };
    let code = json_string(body, "code").unwrap_or_default();
    let mut guard = match store.lock() {
        Ok(guard) => guard,
        Err(_) => return (500, r#"{"error":"mail_failed"}"#.to_string()),
    };
    match auth::verify(&mut guard, &challenge_id, &code, now) {
        AuthReply::Json(status, payload) => (status, payload),
    }
}

/** Issues a ticket for the signed-in desktop. The host fields sit inside `host`. */
fn issue_ticket(body: &str, authorization: &str, store: &Arc<Mutex<Store>>, now: u64) -> (u16, String) {
    let host_id = json_string(body, "hostId").unwrap_or_default();
    let fingerprint = json_string(body, "fingerprint").unwrap_or_default();
    let mut guard = match store.lock() {
        Ok(guard) => guard,
        Err(_) => return (500, error_body("store_failed", "The account store is unavailable.")),
    };
    match auth::issue_mobile_ticket(&mut guard, bearer(authorization), &host_id, &fingerprint, now) {
        AuthReply::Json(status, payload) => (status, payload),
    }
}

/** Cancels the ticket for `host` in the query string. */
fn cancel_ticket(query: &str, authorization: &str, store: &Arc<Mutex<Store>>, now: u64) -> (u16, String) {
    let host_id = query_param(query, "host").unwrap_or_default();
    let mut guard = match store.lock() {
        Ok(guard) => guard,
        Err(_) => return (500, error_body("store_failed", "The account store is unavailable.")),
    };
    match auth::cancel_mobile_ticket(&mut guard, bearer(authorization), &host_id, now) {
        AuthReply::Json(status, payload) => (status, payload),
    }
}

/** Redeems a ticket. Device checks happen before the ticket is spent. */
fn redeem_ticket(body: &str, store: &Arc<Mutex<Store>>, now: u64) -> (u16, String) {
    let ticket = json_string(body, "ticket").unwrap_or_default();
    let device_id = json_string(body, "deviceId").unwrap_or_default();
    let device_name = json_string(body, "deviceName").unwrap_or_default();
    let platform = json_string(body, "platform").unwrap_or_default();
    if ticket.is_empty() || device_id.is_empty() || device_name.is_empty() || platform.is_empty() {
        return (400, error_body("invalid_mobile_ticket", "The Mobile Connect code is invalid."));
    }
    let mut guard = match store.lock() {
        Ok(guard) => guard,
        Err(_) => return (500, error_body("store_failed", "The account store is unavailable.")),
    };
    match auth::redeem_mobile_ticket(&mut guard, &ticket, &device_id, &device_name, &platform, now) {
        AuthReply::Json(status, payload) => (status, payload),
    }
}

/** Registers one desktop host for the signed-in email. */
fn register_host(body: &str, authorization: &str, store: &Arc<Mutex<Store>>, now: u64) -> (u16, String) {
    let host_id = json_string(body, "hostId").unwrap_or_default();
    let name = json_string(body, "name").unwrap_or_default();
    let membership_id = json_string(body, "ownerMembershipId").unwrap_or_default();
    let device_public_key = json_string(body, "devicePublicKey").unwrap_or_default();
    let presented_token = json_string(body, "machineToken").unwrap_or_default();
    let rotate = body.contains("\"rotateCredential\":true") || body.contains("\"rotateCredential\": true");
    let mut guard = match store.lock() {
        Ok(guard) => guard,
        Err(_) => return (500, error_body("store_failed", "The account store is unavailable.")),
    };
    match auth::register_remote_host(
        &mut guard,
        bearer(authorization),
        &host_id,
        &name,
        &membership_id,
        &device_public_key,
        rotate,
        &presented_token,
        now,
    ) {
        AuthReply::Json(status, payload) => (status, payload),
    }
}

/** Lists the desktop hosts owned by the signed-in email. */
fn list_hosts(authorization: &str, store: &Arc<Mutex<Store>>, now: u64) -> (u16, String) {
    let guard = match store.lock() {
        Ok(guard) => guard,
        Err(_) => return (500, error_body("store_failed", "The account store is unavailable.")),
    };
    match auth::list_remote_hosts(&guard, bearer(authorization), now) {
        AuthReply::Json(status, payload) => (status, payload),
    }
}

/** Lists phones for one host. No device row is stored, so a known host is an empty list. */
fn list_devices(query: &str, authorization: &str, store: &Arc<Mutex<Store>>, now: u64) -> (u16, String) {
    let host_id = query_param(query, "host").unwrap_or_default();
    let include_desktop = query_param(query, "includeDesktop").is_some_and(|value| value == "true");
    let guard = match store.lock() {
        Ok(guard) => guard,
        Err(_) => return (500, error_body("store_failed", "The account store is unavailable.")),
    };
    match auth::list_mobile_devices(&guard, bearer(authorization), &host_id, include_desktop, now) {
        AuthReply::Json(status, payload) => (status, payload),
    }
}

/** Reads the bearer token and returns the desktop user. */
fn me(authorization: &str, store: &Arc<Mutex<Store>>, now: u64) -> (u16, String) {
    let token = bearer(authorization);
    let guard = match store.lock() {
        Ok(guard) => guard,
        Err(_) => return (401, r#"{"error":"invalid_token"}"#.to_string()),
    };
    match auth::current_user(&guard, token, now) {
        AuthReply::Json(status, payload) => (status, payload),
    }
}

/** Bearer token, or none when the header is missing or blank. */
fn bearer(authorization: &str) -> Option<&str> {
    authorization.strip_prefix("Bearer ").map(str::trim).filter(|value| !value.is_empty())
}

/** One query value. `+` stays a plus. Percent escapes become bytes when they are valid UTF-8. */
fn query_param(query: &str, key: &str) -> Option<String> {
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        if name == key {
            return Some(percent_decode(value));
        }
    }
    None
}

/** Decodes `%HH` sequences. A bad escape is left unchanged. */
fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_nibble(bytes[index + 1]), hex_nibble(bytes[index + 2])) {
                out.push((hi << 4) | lo);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| value.to_string())
}

/** One hex digit, or none. */
fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/** Error object the desktop and phone decoders accept. */
fn error_body(code: &str, message: &str) -> String {
    format!(r#"{{"error":{{"code":"{code}","message":"{message}"}}}}"#)
}

/** Reads one JSON string field from a flat object. */
fn json_string(body: &str, key: &str) -> Result<String, String> {
    let pattern = format!("\"{key}\"");
    let Some(start) = body.find(&pattern) else {
        return Err("missing".to_string());
    };
    let rest = &body[start + pattern.len()..];
    let Some(colon) = rest.find(':') else {
        return Err("missing".to_string());
    };
    let rest = rest[colon + 1..].trim_start();
    let rest = rest.strip_prefix('"').ok_or_else(|| "missing".to_string())?;
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            if let Some(next) = chars.next() {
                out.push(next);
            }
        } else if ch == '"' {
            return Ok(out);
        } else {
            out.push(ch);
        }
    }
    Err("missing".to_string())
}
