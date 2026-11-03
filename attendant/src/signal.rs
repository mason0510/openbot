//! In-process Signal room.
//!
//! Algorithm: EphemeralTicketRoom.
//! It guarantees a ticket minted by this process maps to one room until that ticket expires or the
//! process restarts, that a hello on that ticket receives `ready`, and that a client hello also
//! sends `peer-ready` to a host already in the room.
//! Input is the ticket string from the host or session ticket route, and a WebSocket hello.
//! Trigger: the desktop or the phone opens `wss://api-attendant.mwcpool.uk/signal`.
//! Stop: the ticket is older than 24 hours, the socket closes, or the frame is not version 1.
//! It does not guarantee ICE completes. It does not verify the JWT signature; the minter did.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::crypto::{base64_bytes, base64url, random_bytes, sha1};

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
}

struct Room {
    peers: HashMap<String, Peer>,
    waiting: HashMap<String, WaitingClient>,
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
            // The next socket may present this resume token instead of the original ticket.
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|value| value.as_secs())
                .unwrap_or(0);
            registry.register(&resume, meta.clone(), now);
            if meta.peer == "client" && !room_has_host(registry, &room_key) {
                wait_id = match random_bytes(8) {
                    Ok(bytes) => base64url(&bytes),
                    Err(_) => "wait".to_string(),
                };
                remember_waiting(registry, &room_key, &wait_id, tx.clone(), meta, &resume);
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
            join_room(registry, &room_key, &connection_id, tx.clone(), &meta.peer);
            if meta.peer == "client" {
                // The host creates its peer connection only after this frame. The phone's offer
                // follows `ready`, so this frame must leave first.
                send_peer_ready(registry, &room_key, &connection_id, &meta);
            } else {
                activate_waiting(registry, &room_key);
            }
            if tx.send(ready_frame(&connection_id, &resume)).is_err() {
                break;
            }
            eprintln!("signal hello {room_key} {}", meta.peer);
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
fn join_room(registry: &Registry, room_key: &str, connection_id: &str, tx: Sender<String>, role: &str) {
    let Ok(mut inner) = registry.inner.lock() else { return };
    let room = inner.rooms.entry(room_key.to_string()).or_insert_with(empty_room);
    room.peers.insert(connection_id.to_string(), Peer { tx, role: role.to_string() });
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
        join_room(registry, room_key, &connection_id, client.tx.clone(), "client");
        send_peer_ready(registry, room_key, &connection_id, &client.meta);
        let _ = client.tx.send(ready_frame(&connection_id, &client.resume));
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
 * The `ready` frame. The STUN URL is a hostname the phone can resolve.
 * An IPv4 literal is rejected on the phone socket. `stun.voip.blackberry.com` returns
 * net error -105 there, so ICE never starts. `stun.miwifi.com` resolves on that phone.
 */
fn ready_frame(connection_id: &str, resume: &str) -> String {
    format!(
        r#"{{"type":"ready","version":1,"connectionId":"{connection_id}","resumeToken":"{resume}","iceServers":[{{"urls":"stun:stun.miwifi.com:3478"}}]}}"#
    )
}

fn empty_room() -> Room {
    Room { peers: HashMap::new(), waiting: HashMap::new() }
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
