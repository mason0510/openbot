//! In-process Signal room.
//!
//! Algorithm: EphemeralTicketRoom.
//! It guarantees a ticket minted by this process maps to one room until that ticket expires or the
//! process restarts, that a hello on that ticket receives `ready`, and that a client hello also
//! sends `peer-ready` to a host already in the room. A client `ready` waits until the host has
//! published one `address-update`, so the first `iceServers` list is not the empty list from before
//! that frame. An update that lists no address still releases the wait.
//! Input is the ticket string from the host or session ticket route, and a WebSocket hello.
//! Trigger: the desktop or the phone opens `wss://api-attendant.mwcpool.uk/signal`.
//! Stop: the ticket is older than 24 hours, the socket closes, or the frame is not version 1.
//! It does not guarantee ICE completes. It does not verify the JWT signature; the minter did.

use std::collections::HashMap;
use std::env;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use crate::crypto::{base64_bytes, base64url, hmac_sha1, random_bytes, sha1};

/** Matches `SIGNAL_TURN_CREDENTIAL_TTL_SECONDS` in packages/contracts. */
const TURN_CREDENTIAL_TTL_SECS: u64 = 60 * 60;

/**
 * Coturn long-term credentials for Attendant Signal, same shape as remote `tokens.iceServers()`.
 * Loaded once from TURN_HOST + TURN_SHARED_SECRET (optional TURN_PORT / TURN_TLS_PORT /
 * TURN_CREDENTIAL_TTL_SECONDS). When unset, `ready` keeps LAN STUN only.
 */
struct TurnConfig {
    host: String,
    secret: String,
    port: u16,
    tls_port: u16,
    credential_ttl_secs: u64,
}

static TURN_CONFIG: OnceLock<Option<TurnConfig>> = OnceLock::new();

const WS_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
const TICKET_TTL_SECS: u64 = 86_400;

/** What the minter stored for one ticket. A host ticket has empty session fields. */
#[derive(Clone)]
pub struct TicketMeta {
    pub host_id: String,
    pub peer: String,
    pub session_id: String,
    pub user_id: String,
    pub membership_id: String,
    pub role: String,
    pub session_expires_at: u64,
}

struct Ticket {
    meta: TicketMeta,
    created: u64,
}

struct Peer {
    tx: Sender<String>,
    role: String,
    meta: TicketMeta,
}

struct Room {
    peers: HashMap<String, Peer>,
    waiting: HashMap<String, WaitingClient>,
    /// One IPv4 and one non-link-local IPv6 from the host's latest `address-update`.
    addresses: Vec<String>,
    /// True after one `address-update`, including an empty list. Clients wait until this is set.
    published: bool,
}

struct WaitingClient {
    tx: Sender<String>,
    meta: TicketMeta,
    resume: String,
}

struct Inner {
    tickets: HashMap<String, Ticket>,
    rooms: HashMap<String, Room>,
}

/** Rooms for tickets this process minted. Lost on restart. */
pub struct Registry {
    inner: Mutex<Inner>,
}

impl Registry {
    /** Empty registry. No tickets exist until `register` runs. */
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner { tickets: HashMap::new(), rooms: HashMap::new() }),
        }
    }

    /**
     * register: Remembers a minted ticket for one host.
     * Guarantees: a later hello with this ticket can join that host room for 24 hours.
     * Refusals: none. A second register of the same ticket replaces the row.
     * Boundaries: not written to the TSV. A restart forgets it.
     */
    pub fn register(&self, ticket: &str, meta: TicketMeta, now: u64) {
        let Ok(mut inner) = self.inner.lock() else { return };
        inner.tickets.insert(ticket.to_string(), Ticket { meta, created: now });
    }
}


/**
 * turn_health_json: Reports whether TURN env is loaded for Attendant Signal.
 * Guarantees: never includes TURN_SHARED_SECRET. `configured` is true only when both
 * TURN_HOST and TURN_SHARED_SECRET were present and accepted at first use.
 */
pub fn turn_health_json() -> String {
    match turn_config() {
        Some(cfg) => format!(
            r#"{{"status":"ok","configured":true,"host":"{}","port":{},"tlsPort":{},"credentialTtlSeconds":{}}}"#,
            json_escape(&cfg.host),
            cfg.port,
            cfg.tls_port,
            cfg.credential_ttl_secs
        ),
        None => r#"{"status":"ok","configured":false}"#.to_string(),
    }
}

fn turn_config() -> Option<&'static TurnConfig> {
    TURN_CONFIG
        .get_or_init(|| {
            let loaded = load_turn_config();
            match &loaded {
                Some(cfg) => eprintln!(
                    "signal turn configured host={} port={} tlsPort={} ttl={}s",
                    cfg.host, cfg.port, cfg.tls_port, cfg.credential_ttl_secs
                ),
                None => eprintln!("signal turn unset; ready uses LAN STUN only when the host publishes addresses"),
            }
            loaded
        })
        .as_ref()
}

fn load_turn_config() -> Option<TurnConfig> {
    let host = env::var("TURN_HOST").ok().map(|value| value.trim().to_string()).filter(|value| !value.is_empty());
    let secret = env::var("TURN_SHARED_SECRET")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    match (host, secret) {
        (Some(host), Some(secret)) => {
            if !is_turn_host(&host) {
                eprintln!("signal turn ignored: TURN_HOST is not a hostname");
                return None;
            }
            let port = env::var("TURN_PORT").ok().and_then(|value| value.parse().ok()).unwrap_or(3478);
            let tls_port = env::var("TURN_TLS_PORT").ok().and_then(|value| value.parse().ok()).unwrap_or(5349);
            let credential_ttl_secs = env::var("TURN_CREDENTIAL_TTL_SECONDS")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(TURN_CREDENTIAL_TTL_SECS);
            if port == 0 || tls_port == 0 || credential_ttl_secs == 0 {
                eprintln!("signal turn ignored: TURN_PORT / TURN_TLS_PORT / TURN_CREDENTIAL_TTL_SECONDS must be positive");
                return None;
            }
            Some(TurnConfig {
                host,
                secret,
                port,
                tls_port,
                credential_ttl_secs,
            })
        }
        (None, None) => None,
        _ => {
            eprintln!("signal turn ignored: set both TURN_HOST and TURN_SHARED_SECRET, or neither");
            None
        }
    }
}

fn is_turn_host(host: &str) -> bool {
    if host.is_empty() || host.len() > 253 {
        return false;
    }
    // Hostnames only (same idea as remote config). Raw IPv4 is refused so phones resolve one name.
    if host.chars().all(|ch| ch.is_ascii_digit() || ch == '.') {
        return false;
    }
    host.chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '.')
        && !host.starts_with('.')
        && !host.ends_with('.')
}

fn json_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}


fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0)
}

/** A host ticket. Session fields stay empty because a host does not get `peer-ready`. */
pub fn host_meta(host_id: &str) -> TicketMeta {
    TicketMeta {
        host_id: host_id.to_string(),
        peer: "host".to_string(),
        session_id: String::new(),
        user_id: String::new(),
        membership_id: String::new(),
        role: String::new(),
        session_expires_at: 0,
    }
}

/**
 * serve: Completes one WebSocket upgrade and runs the socket until it closes.
 * Guarantees: a hello for a live ticket gets `ready` when its peer can negotiate, and a client
 * hello sends `peer-ready` to each host socket in the room before that `ready`.
 * Refusals: missing key, bad hello, unknown or expired ticket, or a hello peer that does not match the ticket.
 * Boundaries: relay frames are forwarded unchanged. It does not speak ingress.
 */
pub fn serve(mut stream: TcpStream, key: &str, registry: &Arc<Registry>) -> Result<(), String> {
    let accept = base64_bytes(&sha1(format!("{key}{WS_GUID}").as_bytes()));
    let response = format!(
        "HTTP/1.1 101 Switching Protocols\r\nupgrade: websocket\r\nconnection: upgrade\r\nsec-websocket-accept: {accept}\r\n\r\n"
    );
    stream.write_all(response.as_bytes()).map_err(|error| error.to_string())?;
    stream.set_read_timeout(None).ok();
    stream.set_write_timeout(Some(Duration::from_secs(20))).ok();
    let mut writer = stream.try_clone().map_err(|error| error.to_string())?;
    let (tx, rx) = mpsc::channel::<String>();
    thread::spawn(move || {
        for payload in rx {
            if write_text(&mut writer, payload.as_bytes()).is_err() {
                break;
            }
        }
    });
    let mut room_key = String::new();
    let mut connection_id = String::new();
    let mut wait_id = String::new();
    let mut hello_done = false;
    let mut peer_meta: Option<TicketMeta> = None;
    // Touch TURN config once so startup logs say whether relays are armed.
    let _ = turn_config();
    loop {
        let Some(text) = read_text(&mut stream)? else { break };
        let kind = json_type(&text);
        if !hello_done {
            if kind != "hello" {
                let _ = tx.send(error_frame("invalid_message", "A hello is required."));
                break;
            }
            let token = json_field(&text, "token").unwrap_or_default();
            let Some(meta) = live_ticket(registry, &token) else {
                let _ = tx.send(error_frame("authentication_required", "The Signal ticket is not valid."));
                break;
            };
            let announced = json_field(&text, "peer").unwrap_or_default();
            if announced != meta.peer {
                let _ = tx.send(error_frame("invalid_message", "The hello peer does not match the ticket."));
                break;
            }
            let resume = match random_bytes(16) {
                Ok(bytes) => base64url(&bytes),
                Err(_) => "resume".to_string(),
            };
            room_key = meta.host_id.clone();
            hello_done = true;
            peer_meta = Some(meta.clone());
            // The next socket may present this resume token instead of the original ticket.
            let now = now_unix_secs();
            registry.register(&resume, meta.clone(), now);
            if meta.peer == "client" && (!room_has_host(registry, &room_key) || !room_published(registry, &room_key)) {
                wait_id = match random_bytes(8) {
                    Ok(bytes) => base64url(&bytes),
                    Err(_) => "wait".to_string(),
                };
                remember_waiting(registry, &room_key, &wait_id, tx.clone(), meta.clone(), &resume);
                let waiting = format!(r#"{{"type":"host-waiting","version":1,"resumeToken":"{resume}"}}"#);
                if tx.send(waiting).is_err() {
                    break;
                }
                eprintln!("signal host-waiting {room_key}");
                continue;
            }
            connection_id = match random_bytes(12) {
                Ok(bytes) => base64url(&bytes),
                Err(_) => "conn".to_string(),
            };
            join_room(registry, &room_key, &connection_id, tx.clone(), &meta);
            if meta.peer == "client" {
                // The host creates its peer connection only after this frame. The phone's offer
                // follows `ready`, so this frame must leave first.
                send_peer_ready(registry, &room_key, &connection_id, &meta);
            } else if room_published(registry, &room_key) {
                // A reconnect can publish again. Waiting clients must not get the empty list from
                // before the first address-update of this process.
                activate_waiting(registry, &room_key);
            }
            if tx
                .send(ready_frame(
                    &connection_id,
                    &resume,
                    &room_addresses(registry, &room_key),
                    &meta,
                ))
                .is_err()
            {
                break;
            }
            eprintln!("signal hello {room_key} {}", meta.peer);
            continue;
        }
        if kind == "turn-refresh" && hello_done {
            // Same idea as remote Signal: remint coturn credentials before CreatePermission 403
            // when TTL ages out (#4736). LAN STUN list is unchanged.
            let Some(meta) = peer_meta.as_ref() else {
                let _ = tx.send(error_frame("authentication_required", "Authenticate before refreshing TURN."));
                break;
            };
            let resume = match random_bytes(16) {
                Ok(bytes) => base64url(&bytes),
                Err(_) => "resume".to_string(),
            };
            registry.register(&resume, meta.clone(), now_unix_secs());
            let frame = if connection_id.is_empty() {
                format!(
                    r#"{{"type":"ready","version":1,"connectionId":null,"resumeToken":"{resume}","iceServers":[{}]}}"#,
                    ice_servers_json(&room_addresses(registry, &room_key), meta)
                )
            } else {
                ready_frame(&connection_id, &resume, &room_addresses(registry, &room_key), meta)
            };
            if tx.send(frame).is_err() {
                break;
            }
            eprintln!("signal turn-refresh {room_key}");
            continue;
        }
        if kind == "address-update" && hello_done {
            if meta_is_host(registry, &room_key, &connection_id) {
                remember_addresses(registry, &room_key, &json_string_list(&text, "addresses"));
                activate_waiting(registry, &room_key);
                push_ice_servers(registry, &room_key);
            }
            continue;
        }
        if matches!(kind.as_str(), "offer" | "answer" | "ice-candidate" | "ice-restart") {
            relay(registry, &room_key, &connection_id, &text);
        }
    }
    if !wait_id.is_empty() {
        forget_waiting(registry, &room_key, &wait_id);
    }
    leave_room(registry, &room_key, &connection_id);
    Ok(())
}

/** Ticket row that is still inside 24 hours, or none. */
fn live_ticket(registry: &Registry, ticket: &str) -> Option<TicketMeta> {
    let inner = registry.inner.lock().ok()?;
    let row = inner.tickets.get(ticket)?;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|value| value.as_secs()).unwrap_or(0);
    if row.created.saturating_add(TICKET_TTL_SECS) <= now {
        return None;
    }
    Some(row.meta.clone())
}

/** Whether a host socket is already in this room. */
fn room_has_host(registry: &Registry, room_key: &str) -> bool {
    let Ok(inner) = registry.inner.lock() else { return false };
    inner
        .rooms
        .get(room_key)
        .is_some_and(|room| room.peers.values().any(|peer| peer.role == "host"))
}

/** Adds this socket to the host room. */
fn join_room(registry: &Registry, room_key: &str, connection_id: &str, tx: Sender<String>, meta: &TicketMeta) {
    let Ok(mut inner) = registry.inner.lock() else { return };
    let room = inner.rooms.entry(room_key.to_string()).or_insert_with(empty_room);
    room.peers.insert(
        connection_id.to_string(),
        Peer {
            tx,
            role: meta.peer.clone(),
            meta: meta.clone(),
        },
    );
}

/** Holds a phone socket until a host hello can give it a connection id. */
fn remember_waiting(registry: &Registry, room_key: &str, wait_id: &str, tx: Sender<String>, meta: TicketMeta, resume: &str) {
    let Ok(mut inner) = registry.inner.lock() else { return };
    let room = inner.rooms.entry(room_key.to_string()).or_insert_with(empty_room);
    room.waiting.insert(
        wait_id.to_string(),
        WaitingClient { tx, meta, resume: resume.to_string() },
    );
}

/** Drops a phone that closed before the host arrived. */
fn forget_waiting(registry: &Registry, room_key: &str, wait_id: &str) {
    if room_key.is_empty() || wait_id.is_empty() {
        return;
    }
    let Ok(mut inner) = registry.inner.lock() else { return };
    let Some(room) = inner.rooms.get_mut(room_key) else { return };
    room.waiting.remove(wait_id);
    if room.peers.is_empty() && room.waiting.is_empty() {
        inner.rooms.remove(room_key);
    }
}

/**
 * activate_waiting: Gives each waiting phone a connection and tells the host.
 * Guarantees: `peer-ready` is sent before that phone's `ready`.
 * Refusals: none. A dead socket is skipped.
 * Boundaries: does not create a WebRTC peer. The desktop does that on `peer-ready`.
 */
fn activate_waiting(registry: &Registry, room_key: &str) {
    let waiting = {
        let Ok(mut inner) = registry.inner.lock() else { return };
        let Some(room) = inner.rooms.get_mut(room_key) else { return };
        std::mem::take(&mut room.waiting)
    };
    for (_, client) in waiting {
        let connection_id = match random_bytes(12) {
            Ok(bytes) => base64url(&bytes),
            Err(_) => "conn".to_string(),
        };
        join_room(registry, room_key, &connection_id, client.tx.clone(), &client.meta);
        send_peer_ready(registry, room_key, &connection_id, &client.meta);
        let _ = client.tx.send(ready_frame(
            &connection_id,
            &client.resume,
            &room_addresses(registry, room_key),
            &client.meta,
        ));
        eprintln!("signal activate {room_key}");
    }
}

/** Tells every host socket that this client can be answered. */
fn send_peer_ready(registry: &Registry, room_key: &str, connection_id: &str, meta: &TicketMeta) {
    let frame = format!(
        r#"{{"type":"peer-ready","version":1,"connectionId":"{connection_id}","sessionId":"{}","userId":"{}","membershipId":"{}","role":"{}","sessionExpiresAt":{},"resumed":false}}"#,
        meta.session_id, meta.user_id, meta.membership_id, meta.role, meta.session_expires_at
    );
    let Ok(inner) = registry.inner.lock() else { return };
    let Some(room) = inner.rooms.get(room_key) else { return };
    for peer in room.peers.values() {
        if peer.role == "host" {
            let _ = peer.tx.send(frame.clone());
        }
    }
    eprintln!("signal peer-ready {room_key}");
}

/**
 * The `ready` frame.
 * Guarantees `iceServers` contains official TURN (when TURN_HOST + TURN_SHARED_SECRET are set)
 * plus at most one IPv4 and one non-link-local IPv6 LAN STUN from the host's latest address
 * update. Does not invent a LAN address when the host has not published that family. When TURN
 * env is unset, the list is LAN STUN only (or empty).
 */
fn ready_frame(connection_id: &str, resume: &str, addresses: &[String], meta: &TicketMeta) -> String {
    format!(
        r#"{{"type":"ready","version":1,"connectionId":"{connection_id}","resumeToken":"{resume}","iceServers":[{}]}}"#,
        ice_servers_json(addresses, meta)
    )
}

/** Tells every client in the room that the host published a new address list. */
fn push_ice_servers(registry: &Registry, room_key: &str) {
    let addresses = room_addresses(registry, room_key);
    let clients: Vec<(Sender<String>, TicketMeta)> = {
        let Ok(inner) = registry.inner.lock() else { return };
        let Some(room) = inner.rooms.get(room_key) else { return };
        room.peers
            .values()
            .filter(|peer| peer.role == "client")
            .map(|peer| (peer.tx.clone(), peer.meta.clone()))
            .collect()
    };
    let mut turn = false;
    for (tx, meta) in clients {
        let servers = ice_servers_json(&addresses, &meta);
        turn = turn || servers.contains("turn:");
        let frame = format!(
            r#"{{"type":"ice-servers","version":1,"connectionId":null,"iceServers":[{servers}]}}"#
        );
        let _ = tx.send(frame);
    }
    eprintln!(
        "signal ice-servers {room_key} v4={} v6={} turn={}",
        addresses.iter().any(|address| !address.contains(':')),
        addresses.iter().any(|address| address.contains(':')),
        turn
    );
}

fn empty_room() -> Room {
    Room { peers: HashMap::new(), waiting: HashMap::new(), addresses: Vec::new(), published: false }
}

/** Whether this process has already handled one address-update for the room. */
fn room_published(registry: &Registry, room_key: &str) -> bool {
    let Ok(inner) = registry.inner.lock() else { return false };
    inner.rooms.get(room_key).is_some_and(|room| room.published)
}

/**
 * Keeps one IPv4 and one non-link-local IPv6.
 * Guarantees later `ready` and `ice-servers` frames use this list.
 * Does not keep link-local or loopback. A family the host omitted stays absent.
 */
fn remember_addresses(registry: &Registry, room_key: &str, raw: &[String]) {
    let mut kept = Vec::new();
    let mut v4 = false;
    let mut v6 = false;
    for address in raw {
        if !v4 && is_ipv4(address) {
            kept.push(address.clone());
            v4 = true;
        } else if !v6 && is_global_ipv6(address) {
            kept.push(address.clone());
            v6 = true;
        }
    }
    let Ok(mut inner) = registry.inner.lock() else { return };
    let Some(room) = inner.rooms.get_mut(room_key) else { return };
    room.addresses = kept;
    room.published = true;
}

fn room_addresses(registry: &Registry, room_key: &str) -> Vec<String> {
    let Ok(inner) = registry.inner.lock() else { return Vec::new() };
    inner.rooms.get(room_key).map(|room| room.addresses.clone()).unwrap_or_default()
}

fn meta_is_host(registry: &Registry, room_key: &str, connection_id: &str) -> bool {
    let Ok(inner) = registry.inner.lock() else { return false };
    inner
        .rooms
        .get(room_key)
        .and_then(|room| room.peers.get(connection_id))
        .is_some_and(|peer| peer.role == "host")
}

/**
 * Builds `iceServers` JSON elements.
 * Official TURN (stun + turn/turns with HMAC-SHA1 credential) comes first when configured.
 * LAN STUN from `address-update` is additive. Username is `expiration:sessionId` (host tickets
 * use `hostId` when sessionId is empty). Credential TTL mirrors remote Signal (default 1h).
 */
fn ice_servers_json(addresses: &[String], meta: &TicketMeta) -> String {
    let mut urls = Vec::new();
    urls.extend(turn_ice_server_entries(meta));
    for address in addresses {
        if is_ipv4(address) {
            urls.push(format!(r#"{{"urls":"stun:{address}:3478"}}"#));
        } else if is_global_ipv6(address) {
            urls.push(format!(r#"{{"urls":"stun:[{address}]:3478"}}"#));
        }
    }
    urls.join(",")
}

fn turn_ice_server_entries(meta: &TicketMeta) -> Vec<String> {
    let Some(cfg) = turn_config() else {
        return Vec::new();
    };
    let now = now_unix_secs();
    let session_deadline = if meta.session_expires_at > now {
        meta.session_expires_at
    } else {
        now.saturating_add(cfg.credential_ttl_secs)
    };
    let expiration = session_deadline.min(now.saturating_add(cfg.credential_ttl_secs));
    if expiration <= now {
        return Vec::new();
    }
    let session_part = if !meta.session_id.is_empty() {
        meta.session_id.as_str()
    } else if !meta.host_id.is_empty() {
        meta.host_id.as_str()
    } else {
        return Vec::new();
    };
    let username = format!("{expiration}:{session_part}");
    let credential = base64_bytes(&hmac_sha1(cfg.secret.as_bytes(), username.as_bytes()));
    let host = json_escape(&cfg.host);
    let port = cfg.port;
    let tls_port = cfg.tls_port;
    vec![
        format!(r#"{{"urls":"stun:{host}:{port}"}}"#),
        format!(
            r#"{{"urls":["turn:{host}:{port}?transport=udp","turn:{host}:{port}?transport=tcp","turns:{host}:{tls_port}?transport=tcp"],"username":"{username}","credential":"{credential}"}}"#
        ),
    ]
}

fn is_ipv4(address: &str) -> bool {
    let octets: Vec<_> = address.split('.').collect();
    octets.len() == 4
        && octets.iter().all(|octet| octet.parse::<u8>().is_ok())
        && address != "0.0.0.0"
        && !address.starts_with("127.")
}

fn is_global_ipv6(address: &str) -> bool {
    address.contains(':')
        && !address.starts_with("fe80:")
        && address != "::"
        && address != "::1"
}

/** Drops this socket. An empty room is removed. The ticket row stays until it expires. */
fn leave_room(registry: &Registry, room_key: &str, connection_id: &str) {
    if room_key.is_empty() || connection_id.is_empty() {
        return;
    }
    let Ok(mut inner) = registry.inner.lock() else { return };
    let Some(room) = inner.rooms.get_mut(room_key) else { return };
    room.peers.remove(connection_id);
    if room.peers.is_empty() && room.waiting.is_empty() {
        inner.rooms.remove(room_key);
    }
}

/**
 * relay: Sends one offer, answer, or ICE frame to the other sockets in the room.
 * Guarantees: the bytes are unchanged.
 * Refusals: none. A full or dead socket is skipped.
 * Boundaries: does not create a peer connection.
 */
fn relay(registry: &Registry, ticket: &str, from_id: &str, frame: &str) {
    let Ok(inner) = registry.inner.lock() else { return };
    let Some(room) = inner.rooms.get(ticket) else { return };
    for (id, peer) in &room.peers {
        if id != from_id {
            let _ = peer.tx.send(frame.to_string());
        }
    }
}

fn error_frame(code: &str, message: &str) -> String {
    format!(r#"{{"type":"error","version":1,"code":"{code}","message":"{message}"}}"#)
}

fn json_type(text: &str) -> String {
    json_field(text, "type").unwrap_or_default()
}

/** Reads a JSON string array. A missing key is an empty list. Does not accept nested objects. */
fn json_string_list(text: &str, key: &str) -> Vec<String> {
    let pattern = format!("\"{key}\"");
    let Some(start) = text.find(&pattern) else { return Vec::new() };
    let Some(bracket) = text[start..].find('[') else { return Vec::new() };
    let rest = &text[start + bracket + 1..];
    let Some(end) = rest.find(']') else { return Vec::new() };
    rest[..end]
        .split(',')
        .filter_map(|part| {
            let part = part.trim().trim_matches('"');
            if part.is_empty() { None } else { Some(part.to_string()) }
        })
        .collect()
}

fn json_field(text: &str, key: &str) -> Option<String> {
    let pattern = format!("\"{key}\"");
    let start = text.find(&pattern)?;
    let rest = text[start + pattern.len()..].trim_start().strip_prefix(':')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            if let Some(next) = chars.next() {
                out.push(next);
            }
        } else if ch == '"' {
            return Some(out);
        } else {
            out.push(ch);
        }
    }
    None
}

/** Writes one unmasked text frame. */
fn write_text(stream: &mut TcpStream, payload: &[u8]) -> Result<(), String> {
    let mut frame = Vec::new();
    frame.push(0x81);
    if payload.len() < 126 {
        frame.push(payload.len() as u8);
    } else if payload.len() <= u16::MAX as usize {
        frame.push(126);
        frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    } else {
        frame.push(127);
        frame.extend_from_slice(&(payload.len() as u64).to_be_bytes());
    }
    frame.extend_from_slice(payload);
    stream.write_all(&frame).map_err(|error| error.to_string())
}

/** Reads one text frame. A close or a non-text opcode ends the socket. Ping is answered. */
fn read_text(stream: &mut TcpStream) -> Result<Option<String>, String> {
    loop {
        let mut header = [0_u8; 2];
        if !read_exact(stream, &mut header)? {
            return Ok(None);
        }
        let opcode = header[0] & 0x0f;
        let masked = header[1] & 0x80 != 0;
        let mut len = (header[1] & 0x7f) as u64;
        if len == 126 {
            let mut ext = [0_u8; 2];
            if !read_exact(stream, &mut ext)? {
                return Ok(None);
            }
            len = u16::from_be_bytes(ext) as u64;
        } else if len == 127 {
            let mut ext = [0_u8; 8];
            if !read_exact(stream, &mut ext)? {
                return Ok(None);
            }
            len = u64::from_be_bytes(ext);
        }
        if len > 65_536 {
            return Err("frame too large".to_string());
        }
        let mut mask = [0_u8; 4];
        if masked && !read_exact(stream, &mut mask)? {
            return Ok(None);
        }
        let mut payload = vec![0_u8; len as usize];
        if !payload.is_empty() && !read_exact(stream, &mut payload)? {
            return Ok(None);
        }
        if masked {
            for (index, byte) in payload.iter_mut().enumerate() {
                *byte ^= mask[index % 4];
            }
        }
        if opcode == 0x8 {
            return Ok(None);
        }
        if opcode == 0x9 {
            let mut pong = vec![0x8a, payload.len() as u8];
            pong.extend_from_slice(&payload);
            stream.write_all(&pong).ok();
            continue;
        }
        if opcode == 0x1 {
            return Ok(Some(String::from_utf8_lossy(&payload).to_string()));
        }
    }
}

fn read_exact(stream: &mut TcpStream, buf: &mut [u8]) -> Result<bool, String> {
    let mut filled = 0;
    while filled < buf.len() {
        match stream.read(&mut buf[filled..]) {
            Ok(0) => return Ok(false),
            Ok(count) => filled += count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(true)
}
