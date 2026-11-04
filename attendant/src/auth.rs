use crate::crypto::{base64url, code_hash, hex_encode, random_bytes, same_bytes, sha256, user_id};
use crate::mail::send_code;
use crate::store::{CodeRecord, RemoteSessionRecord, SessionRecord, Store, CODE_TTL_SECS};

const SESSION_TTL_SECS: u64 = 30 * 24 * 3600;
const RESEND_GAP_SECS: u64 = 60;
const MAX_ATTEMPTS: u8 = 5;
const TICKET_TTL_SECS: u64 = 120;

pub enum AuthReply {
    Json(u16, String),
}

/** Normalizes an email. Chinese text is kept. Tabs, spaces, and controls are rejected. */
pub fn normalize_email(raw: &str) -> Result<String, String> {
    let email = raw.trim().to_lowercase();
    let (local, domain) = email.split_once('@').ok_or_else(|| "invalid_email".to_string())?;
    if local.is_empty()
        || domain.len() < 3
        || !domain.contains('.')
        || email.len() > 254
        || email.chars().any(|ch| ch.is_whitespace() || ch.is_control() || ch == '"' || ch == '\\')
    {
        return Err("invalid_email".to_string());
    }
    Ok(email)
}

/** Starts a sign-in. The code row is written only after SMTP accepts the message.
 * A live challenge inside the resend gap returns the same challenge id and does not send mail.
 * The desktop client requires `challengeId`, `expiresAt`, and `resendAt` in milliseconds.
 */
pub fn start(store: &mut Store, email: &str, from: &str, now: u64) -> AuthReply {
    if let Some(existing) = store.codes.get(email).cloned() {
        if Store::code_live(&existing, now) && existing.attempts >= MAX_ATTEMPTS {
            return AuthReply::Json(503, r#"{"error":"rate_limit"}"#.to_string());
        }
        if Store::code_live(&existing, now) && existing.last_send + RESEND_GAP_SECS > now {
            return AuthReply::Json(200, challenge_json(&existing));
        }
    }
    let code = match eight_code() {
        Ok(code) => code,
        Err(_) => return AuthReply::Json(500, r#"{"error":"mail_failed"}"#.to_string()),
    };
    let salt = match random_bytes(16) {
        Ok(bytes) => bytes,
        Err(_) => return AuthReply::Json(500, r#"{"error":"mail_failed"}"#.to_string()),
    };
    let challenge_id = match random_bytes(32) {
        Ok(bytes) => hex_token(&bytes),
        Err(_) => return AuthReply::Json(500, r#"{"error":"mail_failed"}"#.to_string()),
    };
    let mut salt_bytes = [0_u8; 16];
    salt_bytes.copy_from_slice(&salt);
    if send_code(from, email, &shown_code(&code)).is_err() {
        return AuthReply::Json(500, r#"{"error":"mail_failed"}"#.to_string());
    }
    let row = CodeRecord {
        challenge_id,
        email: email.to_string(),
        salt: salt_bytes,
        hash: code_hash(&salt_bytes, &code),
        created: now,
        attempts: 0,
        last_send: now,
    };
    let body = challenge_json(&row);
    store.codes.insert(email.to_string(), row);
    if store.save().is_err() {
        return AuthReply::Json(500, r#"{"error":"mail_failed"}"#.to_string());
    }
    AuthReply::Json(200, body)
}

/** Checks a challenge and issues a session. A wrong code counts as one attempt.
 * The desktop client sends `challengeId` and `code`, never the email.
 * Success is `sessionToken` plus a user that has a stable `id`.
 */
pub fn verify(store: &mut Store, challenge_id: &str, code: &str, now: u64) -> AuthReply {
    if challenge_id.is_empty() {
        return AuthReply::Json(400, r#"{"error":"invalid_challenge"}"#.to_string());
    }
    let code = match normalize_code(code) {
        Ok(code) => code,
        Err(_) => return AuthReply::Json(400, r#"{"error":"invalid_code"}"#.to_string()),
    };
    let Some(email) = store.email_for_challenge(challenge_id) else {
        return AuthReply::Json(404, r#"{"error":"no_code"}"#.to_string());
    };
    let Some(row) = store.codes.get(&email).cloned() else {
        return AuthReply::Json(404, r#"{"error":"no_code"}"#.to_string());
    };
    if !Store::code_live(&row, now) {
        store.codes.remove(&email);
        let _ = store.save();
        return AuthReply::Json(400, r#"{"error":"code_expired"}"#.to_string());
    }
    if row.attempts >= MAX_ATTEMPTS {
        return AuthReply::Json(429, r#"{"error":"too_many_attempts"}"#.to_string());
    }
    if !same_bytes(&row.hash, &code_hash(&row.salt, &code)) {
        let attempts = row.attempts.saturating_add(1);
        if let Some(found) = store.codes.get_mut(&email) {
            found.attempts = attempts;
        }
        let _ = store.save();
        if attempts >= MAX_ATTEMPTS {
            return AuthReply::Json(429, r#"{"error":"too_many_attempts"}"#.to_string());
        }
        return AuthReply::Json(400, r#"{"error":"invalid_code"}"#.to_string());
    }
    store.codes.remove(&email);
    let token = match random_bytes(32) {
        Ok(bytes) => hex_token(&bytes),
        Err(_) => return AuthReply::Json(500, r#"{"error":"mail_failed"}"#.to_string()),
    };
    store.sessions.insert(
        token.clone(),
        SessionRecord {
            email: email.clone(),
            created: now,
        },
    );
    if store.save().is_err() {
        return AuthReply::Json(500, r#"{"error":"mail_failed"}"#.to_string());
    }
    AuthReply::Json(200, format!(r#"{{"sessionToken":"{token}","user":{}}}"#, user_json(&email)))
}

/** Returns the desktop user for a live token. A token older than 30 days is unauthorized.
 * `/v1/me` is the user object itself, not a wrapper.
 */
pub fn current_user(store: &Store, token: Option<&str>, now: u64) -> AuthReply {
    let Some(token) = token else {
        return AuthReply::Json(401, r#"{"error":"missing_token"}"#.to_string());
    };
    match store.sessions.get(token) {
        Some(row) if row.created + SESSION_TTL_SECS > now => AuthReply::Json(200, user_json(&row.email)),
        _ => AuthReply::Json(401, r#"{"error":"invalid_token"}"#.to_string()),
    }
}

/** Desktop challenge body. Times are Unix milliseconds, matching `Date.now()`. */
fn challenge_json(row: &CodeRecord) -> String {
    let expires_at = row.created.saturating_add(CODE_TTL_SECS).saturating_mul(1000);
    let resend_at = row.last_send.saturating_add(RESEND_GAP_SECS).saturating_mul(1000);
    format!(
        r#"{{"challengeId":"{}","expiresAt":{expires_at},"resendAt":{resend_at}}}"#,
        row.challenge_id
    )
}

/** User object the desktop decoder requires. `name` and `avatarUrl` stay null. */
fn user_json(email: &str) -> String {
    format!(
        r#"{{"id":"{}","email":"{}","name":null,"avatarUrl":null,"analyticsEnabled":null}}"#,
        user_id(email),
        json_escape(email)
    )
}

/// Same 32 characters as the desktop code boxes. `0`, `1`, `I`, and `O` are absent.
const CODE_ALPHABET: &[u8; 32] = b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ";

/** Builds an 8-character code. Each character is uniform over `CODE_ALPHABET`. */
fn eight_code() -> Result<String, String> {
    let bytes = random_bytes(8)?;
    let mut out = String::with_capacity(8);
    for byte in bytes {
        out.push(CODE_ALPHABET[(byte & 31) as usize] as char);
    }
    Ok(out)
}

/** Accepts `ABCD-EFGH` or `abcdefgh`. The stored hash uses the 8-character form. */
fn normalize_code(raw: &str) -> Result<String, String> {
    let mut out = String::with_capacity(8);
    for ch in raw.chars() {
        if ch == '-' || ch.is_whitespace() {
            continue;
        }
        let upper = ch.to_ascii_uppercase();
        if !upper.is_ascii() || !CODE_ALPHABET.contains(&(upper as u8)) {
            return Err("invalid_code".to_string());
        }
        out.push(upper);
    }
    if out.len() != 8 {
        return Err("invalid_code".to_string());
    }
    Ok(out)
}

/** Shows the code as two groups of four, matching the desktop boxes. */
fn shown_code(code: &str) -> String {
    format!("{}-{}", &code[..4], &code[4..])
}

/** Encodes a session token as 64 hex characters. */
fn hex_token(bytes: &[u8]) -> String {
    crate::crypto::hex_encode(bytes)
}

/** Escapes one JSON string. A PEM newline must survive the phone's parser. */
fn json_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(ch),
        }
    }
    out
}

/**
 * device_public_key_pem: Returns a PEM the phone can decode.
 * Guarantees: a key stored by the old parser, which kept the letter n in place of each JSON newline,
 * is restored to a two-line SPKI PEM. A key that already has newlines is unchanged.
 * Refusals: none. An unrecognized value is returned as stored.
 * Boundaries: does not check the signature.
 */
/**
 * stored_public_key: Turns a PEM into one TSV field.
 * Guarantees newlines and carriage returns become the letter n, which `device_public_key_pem` restores.
 * Does not check that the key is a real SPKI. An empty string stays empty.
 */
fn stored_public_key(value: &str) -> String {
    value.replace("\r\n", "n").replace(['\n', '\r'], "n")
}

fn device_public_key_pem(stored: &str) -> String {
    if stored.contains('\n') {
        return stored.to_string();
    }
    const HEADER: &str = "-----BEGIN PUBLIC KEY-----";
    const FOOTER: &str = "-----END PUBLIC KEY-----";
    // The old parser turned each JSON `\n` into the letter n, including one after the footer.
    let text = if stored.ends_with('n') && !stored.ends_with(FOOTER) {
        &stored[..stored.len() - 1]
    } else {
        stored
    };
    let Some(rest) = text.strip_prefix(HEADER) else {
        return stored.to_string();
    };
    let Some(body) = rest.strip_suffix(FOOTER) else {
        return stored.to_string();
    };
    if body.len() >= 2 && body.starts_with('n') && body.ends_with('n') {
        let encoded = &body[1..body.len() - 1];
        if encoded.len() == 60 {
            return format!("{HEADER}\n{encoded}\n{FOOTER}");
        }
    }
    stored.to_string()
}

/** Issues one Mobile Connect ticket for the desktop session.
 * Guarantees a 43-character base64url ticket, `expiresAt` in unix milliseconds, and one live ticket
 * for this email and host id. The plaintext ticket is not written.
 * Does not call a control plane. Does not change an existing session row.
 * A host id already held by another email is rejected.
 */
pub fn issue_mobile_ticket(
    store: &mut Store,
    token: Option<&str>,
    host_id: &str,
    fingerprint: &str,
    now: u64,
) -> AuthReply {
    let Some(email) = session_email(store, token, now) else {
        return api_error(401, "unauthorized", "A desktop session is required.");
    };
    if !valid_host_id(host_id) || !valid_fingerprint(fingerprint) {
        return api_error(
            400,
            "invalid_mobile_host",
            "Update the desktop app to create a host-bound Mobile Connect code.",
        );
    }
    if store.tickets.values().any(|row| row.host_id == host_id && row.email != email) {
        return api_error(409, "mobile_host_mismatch", "The Mobile Connect host identity does not match.");
    }
    let ticket = match random_bytes(32) {
        Ok(bytes) => base64url(&bytes),
        Err(_) => return api_error(500, "store_failed", "The Mobile Connect code could not be created."),
    };
    if ticket.len() != 43 {
        return api_error(500, "store_failed", "The Mobile Connect code could not be created.");
    }
    let hash_hex = hex_encode(&sha256(ticket.as_bytes()));
    store.tickets.retain(|_, row| !(row.email == email && row.host_id == host_id));
    store.tickets.insert(
        hash_hex,
        crate::store::TicketRecord {
            email,
            host_id: host_id.to_string(),
            fingerprint: fingerprint.to_string(),
            created: now,
        },
    );
    if store.save().is_err() {
        return api_error(500, "store_failed", "The Mobile Connect code could not be stored.");
    }
    let expires_at = now.saturating_add(TICKET_TTL_SECS).saturating_mul(1000);
    AuthReply::Json(200, format!(r#"{{"ticket":"{ticket}","expiresAt":{expires_at}}}"#))
}

/**
 * register_remote_host: Inserts a HOST row for the authenticated email with a validated hostId.
 * Guarantee: Returns authEpoch >= 1 and a machine token. The same email and host id updates in place.
 * Refusal: 401 if no valid session, 400 if hostId or name is invalid, 409 if another email owns the host id.
 * Boundary: One hostId belongs to exactly one email. A session row is not changed.
 * A PEM key may contain newlines. Those cannot sit in a TSV field, so they are stored as the letter n
 * and restored by `device_public_key_pem`.
 */
pub fn register_remote_host(
    store: &mut Store,
    token: Option<&str>,
    host_id: &str,
    name: &str,
    membership_id: &str,
    device_public_key: &str,
    rotate: bool,
    presented_token: &str,
    now: u64,
) -> AuthReply {
    let Some(email) = session_email(store, token, now) else {
        return api_error(401, "unauthorized", "A desktop session is required.");
    };
    let device_public_key = stored_public_key(device_public_key);
    if !valid_host_id(host_id) {
        eprintln!("register reject host_id len={}", host_id.len());
        return api_error(400, "invalid_host_id", "The remote host id is invalid.");
    }
    if !valid_host_name(name) {
        eprintln!("register reject name len={}", name.len());
        return api_error(400, "invalid_host_name", "The remote host name is invalid.");
    }
    if !tsv_safe(&device_public_key) {
        eprintln!("register reject key len={}", device_public_key.len());
        return api_error(400, "invalid_host_key", "The remote host key is invalid.");
    }
    let membership_id = if valid_membership_id(membership_id) {
        membership_id.to_string()
    } else {
        hex_encode(&sha256(format!("{email}\n{host_id}").as_bytes()))[..32].to_string()
    };
    if let Some(existing) = store.hosts.get(host_id) {
        if existing.email != email {
            return api_error(409, "remote_host_taken", "This host is registered to another account.");
        }
    }
    let previous = store.hosts.get(host_id).cloned();
    let machine_token = if let Some(existing) = &previous {
        if rotate || existing.machine_token.is_empty() {
            match random_bytes(32) {
                Ok(bytes) => hex_encode(&bytes),
                Err(_) => return api_error(500, "store_failed", "The remote host could not be registered."),
            }
        } else if presented_token.is_empty() || presented_token == existing.machine_token {
            existing.machine_token.clone()
        } else {
            return api_error(401, "stale_machine_token", "The remote host credential does not match.");
        }
    } else {
        match random_bytes(32) {
            Ok(bytes) => hex_encode(&bytes),
            Err(_) => return api_error(500, "store_failed", "The remote host could not be registered."),
        }
    };
    let mut auth_epoch = previous.as_ref().map(|row| row.auth_epoch).unwrap_or(1).max(1);
    if previous.as_ref().is_some_and(|row| row.device_public_key != device_public_key) {
        auth_epoch = auth_epoch.saturating_add(1).max(1);
    }
    let created = previous.as_ref().map(|row| row.created).unwrap_or(now);
    store.hosts.insert(
        host_id.to_string(),
        crate::store::HostRecord {
            email,
            host_id: host_id.to_string(),
            name: name.to_string(),
            membership_id: membership_id.clone(),
            auth_epoch,
            machine_token: machine_token.clone(),
            device_public_key: device_public_key.to_string(),
            created,
        },
    );
    if store.save().is_err() {
        return api_error(500, "store_failed", "The remote host could not be stored.");
    }
    AuthReply::Json(
        200,
        format!(
            r#"{{"hostId":"{host_id}","name":"{}","membershipId":"{membership_id}","authEpoch":{auth_epoch},"machineToken":"{machine_token}"}}"#,
            json_escape(name)
        ),
    )
}

/**
 * list_remote_hosts: Reads HOST rows for the session email.
 * Guarantee: logoKey is null, keyHandovers is empty, and role is owner.
 * Refusal: 401 if no valid session.
 * Boundary: Hosts owned by another email are not listed.
 */
pub fn list_remote_hosts(store: &Store, token: Option<&str>, now: u64) -> AuthReply {
    let Some(email) = session_email(store, token, now) else {
        return api_error(401, "unauthorized", "A desktop session is required.");
    };
    let mut rows: Vec<_> = store.hosts.values().filter(|row| row.email == email).collect();
    rows.sort_by(|left, right| left.host_id.cmp(&right.host_id));
    let mut hosts = String::from("[");
    for (index, row) in rows.iter().enumerate() {
        if index > 0 {
            hosts.push(',');
        }
        let key = if row.device_public_key.is_empty() {
            "null".to_string()
        } else {
            format!("\"{}\"", json_escape(&device_public_key_pem(&row.device_public_key)))
        };
        hosts.push_str(&format!(
            r#"{{"hostId":"{}","name":"{}","logoKey":null,"devicePublicKey":{key},"keyHandovers":[],"authEpoch":{},"membershipId":"{}","role":"owner"}}"#,
            row.host_id,
            json_escape(&row.name),
            row.auth_epoch.max(1),
            row.membership_id
        ));
    }
    hosts.push(']');
    AuthReply::Json(200, format!(r#"{{"hosts":{hosts}}}"#))
}

/**
 * list_host_members: Returns the owner row for one registered host.
 * Guarantee: The signed-in owner gets one active owner member. createdAt is unix milliseconds.
 * Refusal: 401 without a session, 400 for a bad host id, 404 if this email has no such host, 409 if another email owns it.
 * Boundary: Revoked members are not stored, so the list is never a revoked row.
 */
pub fn list_host_members(store: &Store, token: Option<&str>, host_id: &str, now: u64) -> AuthReply {
    let Some(email) = session_email(store, token, now) else {
        return api_error(401, "unauthorized", "A desktop session is required.");
    };
    let Some(row) = owned_host(store, &email, host_id) else {
        return host_lookup_error(store, host_id);
    };
    let created_at = row.created.saturating_mul(1000);
    AuthReply::Json(
        200,
        format!(
            r#"{{"members":[{{"membershipId":"{}","email":"{}","name":"{}","avatarUrl":null,"role":"owner","status":"active","createdAt":{created_at}}}]}}"#,
            row.membership_id,
            json_escape(&row.email),
            json_escape(&row.name)
        ),
    )
}

/**
 * issue_host_signal_ticket: Mints a host Signal ticket for the machine token of that host.
 * Guarantee: Success is ticket, expiresAt in unix milliseconds, and the configured wss signalUrl.
 * Refusal: 400 if the token or host id is empty or the signal URL is not wss, 401 if the token does not match, 404 if the host is unknown.
 * Boundary: Does not open the Signal socket. A desktop that cannot reach signalUrl still will not show a QR code.
 */
pub fn issue_host_signal_ticket(
    store: &Store,
    host_id: &str,
    machine_token: &str,
    signal_url: &str,
    now: u64,
) -> AuthReply {
    if !valid_host_id(host_id) || machine_token.is_empty() || !signal_url.starts_with("wss://") || signal_url.contains('"')
    {
        return api_error(400, "invalid_remote_request", "The host credential is invalid.");
    }
    let Some(row) = store.hosts.get(host_id) else {
        return api_error(404, "remote_host_missing", "This host is not registered.");
    };
    if machine_token != row.machine_token {
        return api_error(401, "stale_machine_token", "The remote host credential does not match.");
    }
    let ticket = match random_bytes(32) {
        Ok(bytes) => base64url(&bytes),
        Err(_) => return api_error(500, "store_failed", "The Signal ticket could not be created."),
    };
    let expires_at = now.saturating_add(86_400).saturating_mul(1000);
    AuthReply::Json(
        200,
        format!(r#"{{"ticket":"{ticket}","expiresAt":{expires_at},"signalUrl":"{signal_url}"}}"#),
    )
}

/**
 * create_mobile_link: Returns a new persistent Mobile Connect token for this host.
 * Guarantee: link matches [A-Za-z0-9_-] and is 43 characters. createdAt is unix milliseconds.
 * Refusal: 401 without a session, 400 if hostId or fingerprint is invalid, 409 if another email owns the host.
 * Boundary: The token is not written to the TSV. The desktop stores the returned link.
 */
pub fn create_mobile_link(store: &Store, token: Option<&str>, host_id: &str, fingerprint: &str, now: u64) -> AuthReply {
    let Some(email) = session_email(store, token, now) else {
        return api_error(401, "unauthorized", "A desktop session is required.");
    };
    if !valid_host_id(host_id) || !valid_fingerprint(fingerprint) {
        return api_error(400, "invalid_mobile_host", "The Mobile Connect host is invalid.");
    }
    if let Some(row) = store.hosts.get(host_id) {
        if row.email != email {
            return api_error(409, "remote_host_taken", "This host is registered to another account.");
        }
    }
    let link = match random_bytes(32) {
        Ok(bytes) => base64url(&bytes),
        Err(_) => return api_error(500, "store_failed", "The Mobile Connect link could not be created."),
    };
    let created_at = now.saturating_mul(1000);
    AuthReply::Json(200, format!(r#"{{"link":"{link}","createdAt":{created_at}}}"#))
}

const REMOTE_SESSION_TTL_SECS: u64 = 86_400;

/**
 * Starts a phone session on one desktop host.
 *
 * Guarantees: the same email and hostId reuse one live session. expiresAt is unix milliseconds,
 * 24 hours after created. Success is HTTP 201.
 * Refuses: 401 without a live bearer, 400 for a bad hostId, 404 for an unknown host, 403 when
 * another email owns the host.
 * Does not write the row to the TSV, and does not issue the Signal ticket.
 */
pub fn start_remote_session(store: &mut Store, token: Option<&str>, host_id: &str, now: u64) -> AuthReply {
    let Some(email) = session_email(store, token, now) else {
        return api_error(401, "unauthorized", "Sign in is required.");
    };
    if !valid_host_id(host_id) {
        return api_error(400, "invalid_remote_request", "The host ID is invalid.");
    }
    let Some(host) = store.hosts.get(host_id) else {
        return api_error(404, "remote_host_missing", "This host is not registered.");
    };
    if host.email != email {
        return api_error(403, "forbidden", "This account cannot open that host.");
    }
    if let Some(record) = store.remote_sessions.values().find(|record| record.email == email && record.host_id == host_id)
    {
        if now < record.created.saturating_add(REMOTE_SESSION_TTL_SECS) {
            let expires_at = record.created.saturating_add(REMOTE_SESSION_TTL_SECS).saturating_mul(1000);
            return AuthReply::Json(
                201,
                format!(
                    r#"{{"sessionId":"{}","hostId":"{}","expiresAt":{expires_at}}}"#,
                    record.session_id, record.host_id
                ),
            );
        }
    }
    store
        .remote_sessions
        .retain(|_, record| record.email != email || record.host_id != host_id);
    let session_id = match random_bytes(16) {
        Ok(bytes) => base64url(&bytes),
        Err(_) => return api_error(500, "store_failed", "The remote session could not be created."),
    };
    store.remote_sessions.insert(
        session_id.clone(),
        RemoteSessionRecord {
            session_id: session_id.clone(),
            host_id: host_id.to_string(),
            email,
            created: now,
        },
    );
    let expires_at = now.saturating_add(REMOTE_SESSION_TTL_SECS).saturating_mul(1000);
    AuthReply::Json(
        201,
        format!(r#"{{"sessionId":"{session_id}","hostId":"{host_id}","expiresAt":{expires_at}}}"#),
    )
}

/** Claims the desktop must see in both the JWT and the Signal `peer-ready` frame. Times are seconds. */
pub struct SessionTicketParts {
    pub host_id: String,
    pub session_id: String,
    pub user_id: String,
    pub membership_id: String,
    pub role: String,
    pub auth_epoch: u64,
    pub session_expires_at: u64,
    pub client_public_key: String,
    pub expires_at_ms: u64,
}

/**
 * session_ticket_parts: Checks that this bearer may open Signal for the session.
 * Guarantees: the parts name the host that owns the session, role `owner`, and the same expiry
 * the desktop will compare. `session_expires_at` is unix seconds.
 * Refuses: 401 without a live bearer, 400 for a bad client key, 403 for a missing or foreign
 * session, 404 when the host row is gone, 500 when Signal is not a wss URL.
 * Does not sign the JWT and does not open the socket.
 */
pub fn session_ticket_parts(
    store: &Store,
    token: Option<&str>,
    session_id: &str,
    client_public_key: &str,
    signal_url: &str,
    now: u64,
) -> Result<SessionTicketParts, AuthReply> {
    let Some(email) = session_email(store, token, now) else {
        return Err(api_error(401, "unauthorized", "Sign in is required."));
    };
    // PEM uses newlines. Other controls are still refused.
    if client_public_key.is_empty()
        || client_public_key.len() > 8_192
        || client_public_key.chars().any(|ch| ch.is_control() && ch != '\n' && ch != '\r')
    {
        return Err(api_error(400, "invalid_remote_request", "The client public key is required."));
    }
    if !signal_url.starts_with("wss://") || signal_url.contains('"') {
        return Err(api_error(500, "store_failed", "The Signal address is not configured."));
    }
    let Some(record) = store.remote_sessions.get(session_id) else {
        return Err(api_error(403, "session_inactive", "The remote session is not active."));
    };
    if record.email != email || now >= record.created.saturating_add(REMOTE_SESSION_TTL_SECS) {
        return Err(api_error(403, "session_inactive", "The remote session is not active."));
    }
    let Some(host) = store.hosts.get(&record.host_id) else {
        return Err(api_error(404, "remote_host_missing", "This host is not registered."));
    };
    let session_expires_at = record.created.saturating_add(REMOTE_SESSION_TTL_SECS);
    Ok(SessionTicketParts {
        host_id: record.host_id.clone(),
        session_id: record.session_id.clone(),
        user_id: user_id(&email),
        membership_id: host.membership_id.clone(),
        role: "owner".to_string(),
        auth_epoch: host.auth_epoch,
        session_expires_at,
        client_public_key: client_public_key.to_string(),
        expires_at_ms: session_expires_at.saturating_mul(1000),
    })
}

/**
 * Ends one phone session.
 *
 * Guarantees: a bearer that owns the session removes it and returns HTTP 204. An unknown session
 * also returns 204, so a retry does not fail.
 * Refuses: 401 without a live bearer, 403 when another account owns the session.
 * Does not close an open Signal socket.
 */
pub fn end_remote_session(store: &mut Store, token: Option<&str>, session_id: &str, now: u64) -> AuthReply {
    let Some(email) = session_email(store, token, now) else {
        return api_error(401, "unauthorized", "Sign in is required.");
    };
    if let Some(record) = store.remote_sessions.get(session_id) {
        if record.email != email {
            return api_error(403, "forbidden", "This account cannot end that session.");
        }
    }
    store.remote_sessions.remove(session_id);
    AuthReply::Json(204, String::new())
}

/**
 * Returns the signed-in phone user.
 *
 * Guarantees: a live bearer gets the same user object as `/v1/me`.
 * Refuses: 401 when the bearer is missing or expired.
 */
pub fn mobile_session(store: &Store, token: Option<&str>, now: u64) -> AuthReply {
    current_user(store, token, now)
}

/** A registered host owned by this email, or none. */
fn owned_host<'a>(store: &'a Store, email: &str, host_id: &str) -> Option<&'a crate::store::HostRecord> {
    let row = store.hosts.get(host_id)?;
    if row.email == email { Some(row) } else { None }
}

/** 400, 404, or 409 for a host id that this session cannot list. */
fn host_lookup_error(store: &Store, host_id: &str) -> AuthReply {
    if !valid_host_id(host_id) {
        return api_error(400, "invalid_mobile_host", "The Mobile Connect host ID is invalid.");
    }
    if store.hosts.contains_key(host_id) {
        return api_error(409, "remote_host_taken", "This host is registered to another account.");
    }
    api_error(404, "remote_host_missing", "This host is not registered.")
}

/**
 * list_mobile_devices: Returns an empty device list. This store has no device columns.
 * Guarantee: A valid session and a well-formed host id get {"devices":[]}. includeDesktop with no host gets {"sessions":[]}.
 * Refusal: 401 if no session, 400 if the host id is malformed, 409 if another email owns that host id.
 * Boundary: A host id that is not registered yet is an empty device list, so registration can follow this call.
 */
pub fn list_mobile_devices(
    store: &Store,
    token: Option<&str>,
    host_id: &str,
    include_desktop: bool,
    now: u64,
) -> AuthReply {
    if session_email(store, token, now).is_none() {
        return api_error(401, "unauthorized", "A desktop session is required.");
    }
    if include_desktop && host_id.is_empty() {
        return AuthReply::Json(200, r#"{"sessions":[]}"#.to_string());
    }
    if !valid_host_id(host_id) {
        return api_error(400, "invalid_mobile_host", "The Mobile Connect host ID is invalid.");
    }
    let email = session_email(store, token, now).unwrap_or_default();
    if store.hosts.get(host_id).is_some_and(|row| row.email != email) {
        return api_error(409, "remote_host_taken", "This host is registered to another account.");
    }
    AuthReply::Json(200, r#"{"devices":[]}"#.to_string())
}

/** Removes the live ticket for this desktop host. A missing ticket is still success.
 * Does not remove a ticket that belongs to another email.
 */
pub fn cancel_mobile_ticket(store: &mut Store, token: Option<&str>, host_id: &str, now: u64) -> AuthReply {
    let Some(email) = session_email(store, token, now) else {
        return api_error(401, "unauthorized", "A desktop session is required.");
    };
    if !valid_host_id(host_id) {
        return api_error(400, "invalid_mobile_host", "The Mobile Connect host ID is invalid.");
    }
    let before = store.tickets.len();
    store.tickets.retain(|_, row| !(row.email == email && row.host_id == host_id));
    if store.tickets.len() != before && store.save().is_err() {
        return api_error(500, "store_failed", "The Mobile Connect code could not be cancelled.");
    }
    AuthReply::Json(204, String::new())
}

/** Exchanges one ticket for a mobile session bound to the stored host.
 * A bad device is rejected before the ticket is spent. An unknown, expired, or second use returns 401
 * and does not insert a session. The user id stays the SHA-256 of the email.
 */
pub fn redeem_mobile_ticket(
    store: &mut Store,
    ticket: &str,
    device_id: &str,
    device_name: &str,
    platform: &str,
    now: u64,
) -> AuthReply {
    if !valid_ticket(ticket) {
        return api_error(401, "invalid_mobile_ticket", "The Mobile Connect code is invalid or expired.");
    }
    if !matches!(platform, "ios" | "android" | "unknown") {
        return api_error(400, "invalid_mobile_ticket", "The Mobile Connect code is invalid.");
    }
    if !is_uuid_v4(device_id) {
        return api_error(400, "invalid_mobile_device", "The mobile device ID is invalid.");
    }
    if !valid_device_name(device_name) {
        return api_error(400, "invalid_mobile_device", "The mobile device name is invalid.");
    }
    let hash_hex = hex_encode(&sha256(ticket.as_bytes()));
    let Some(row) = store.tickets.get(&hash_hex).cloned() else {
        return api_error(401, "invalid_mobile_ticket", "The Mobile Connect code is invalid or expired.");
    };
    store.tickets.remove(&hash_hex);
    if row.created.saturating_add(TICKET_TTL_SECS) <= now {
        let _ = store.save();
        return api_error(401, "invalid_mobile_ticket", "The Mobile Connect code is invalid or expired.");
    }
    let token = match random_bytes(32) {
        Ok(bytes) => hex_token(&bytes),
        Err(_) => {
            store.tickets.insert(hash_hex, row);
            return api_error(500, "store_failed", "The mobile session could not be created.");
        }
    };
    store.sessions.insert(
        token.clone(),
        SessionRecord {
            email: row.email.clone(),
            created: now,
        },
    );
    if store.save().is_err() {
        return api_error(500, "store_failed", "The mobile session could not be stored.");
    }
    AuthReply::Json(
        200,
        format!(
            r#"{{"sessionToken":"{token}","user":{},"host":{{"hostId":"{}","fingerprint":"{}"}}}}"#,
            user_json(&row.email),
            row.host_id,
            row.fingerprint
        ),
    )
}

/** Nested error body. The desktop decoder ignores a bare string. */
fn api_error(status: u16, code: &str, message: &str) -> AuthReply {
    AuthReply::Json(status, format!(r#"{{"error":{{"code":"{code}","message":"{message}"}}}}"#))
}

/** Email of a session that is still inside 30 days. */
fn session_email(store: &Store, token: Option<&str>, now: u64) -> Option<String> {
    let token = token?;
    let row = store.sessions.get(token)?;
    if row.created.saturating_add(SESSION_TTL_SECS) <= now {
        return None;
    }
    Some(row.email.clone())
}

/** A host name that can sit in one TSV field and one JSON string. */
fn valid_host_name(value: &str) -> bool {
    !value.is_empty() && value.len() <= 80 && tsv_safe(value)
}

/** Membership ids echoed from the desktop, or rejected so a derived id is used. */
fn valid_membership_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

/** True when a field cannot break a TSV row or a JSON string. */
fn tsv_safe(value: &str) -> bool {
    value.len() <= 4096 && !value.chars().any(|ch| ch == '\t' || ch == '\n' || ch == '\r' || ch == '"' || ch == '\\')
}

/** Host id accepted by the phone QR parser. */
fn valid_host_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128 && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

/** Fingerprint is exactly 43 base64url characters. */
fn valid_fingerprint(value: &str) -> bool {
    value.len() == 43 && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

/** Ticket charset used by the phone. Length 32 to 128. */
fn valid_ticket(value: &str) -> bool {
    (32..=128).contains(&value.len())
        && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

/** UUID version 4, the id the phone stores for this install. */
fn is_uuid_v4(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 36 {
        return false;
    }
    for (index, byte) in bytes.iter().enumerate() {
        let ok = match index {
            8 | 13 | 18 | 23 => *byte == b'-',
            14 => *byte == b'4',
            19 => matches!(byte, b'8' | b'9' | b'a' | b'b' | b'A' | b'B'),
            _ => byte.is_ascii_hexdigit(),
        };
        if !ok {
            return false;
        }
    }
    true
}

/** Device name after trim: 1 to 80 characters and no control character. */
fn valid_device_name(raw: &str) -> bool {
    let name = raw.trim();
    let count = name.chars().count();
    (1..=80).contains(&count) && !name.chars().any(|ch| ch.is_control())
}

