use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

use crate::crypto::{hex_decode, hex_encode};

pub const CODE_TTL_SECS: u64 = 600;

/** One pending sign-in code. The plaintext code is never stored. The map key is the email. */
#[derive(Clone)]
pub struct CodeRecord {
    pub challenge_id: String,
    pub email: String,
    pub salt: [u8; 16],
    pub hash: [u8; 32],
    pub created: u64,
    pub attempts: u8,
    pub last_send: u64,
}

/** One live session. `created` plus 30 days is the expiry. */
pub struct SessionRecord {
    pub email: String,
    pub created: u64,
}

/** One Mobile Connect ticket. The plaintext ticket is never stored. The map key is the SHA-256 hex. */
#[derive(Clone)]
pub struct TicketRecord {
    pub email: String,
    pub host_id: String,
    pub fingerprint: String,
    pub created: u64,
}

/** One desktop host owned by an email. The map key is `host_id`. */
#[derive(Clone)]
pub struct HostRecord {
    pub email: String,
    pub host_id: String,
    pub name: String,
    pub membership_id: String,
    pub auth_epoch: u64,
    pub machine_token: String,
    pub device_public_key: String,
    pub created: u64,
}

pub struct Store {
    path: PathBuf,
    _lock: File,
    pub codes: HashMap<String, CodeRecord>,
    pub sessions: HashMap<String, SessionRecord>,
    pub tickets: HashMap<String, TicketRecord>,
    pub hosts: HashMap<String, HostRecord>,
}

/** Opens the lock file and holds it until the process exits. A second process cannot start. */
pub fn open_store(data_dir: &Path) -> Result<Store, String> {
    fs::create_dir_all(data_dir).map_err(|error| error.to_string())?;
    let lock_path = data_dir.join(".accounts.tsv.lock");
    let lock = OpenOptions::new().create(true).read(true).write(true).open(&lock_path).map_err(|error| error.to_string())?;
    let result = unsafe { flock(lock.as_raw_fd(), 2) };
    if result != 0 {
        return Err("account file is locked".to_string());
    }
    let path = data_dir.join("accounts.tsv");
    let mut store = Store {
        path,
        _lock: lock,
        codes: HashMap::new(),
        sessions: HashMap::new(),
        tickets: HashMap::new(),
        hosts: HashMap::new(),
    };
    store.load()?;
    Ok(store)
}

/** Loads CODE, SESS, and TICKET rows. A line that is not the current format is skipped.
 * A CODE row is `CODE`, challenge id, email, salt, hash, created, attempts, last send.
 * A TICKET row is `TICKET`, email, host id, fingerprint, hash, created seconds.
 * A HOST row is `HOST`, email, host id, name, membership id, auth epoch, machine token, device key, created.
 * Older rows without a challenge id are dropped. Session columns stay four fields.
 */
fn load_rows(store: &mut Store) -> Result<(), String> {
    let text = fs::read_to_string(&store.path).unwrap_or_default();
    for line in text.lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.first() == Some(&"CODE") && parts.len() == 8 {
            let salt = hex_decode(parts[3])?;
            let hash = hex_decode(parts[4])?;
            if salt.len() != 16 || hash.len() != 32 || parts[1].is_empty() {
                continue;
            }
            let mut salt_bytes = [0_u8; 16];
            let mut hash_bytes = [0_u8; 32];
            salt_bytes.copy_from_slice(&salt);
            hash_bytes.copy_from_slice(&hash);
            store.codes.insert(
                parts[2].to_string(),
                CodeRecord {
                    challenge_id: parts[1].to_string(),
                    email: parts[2].to_string(),
                    salt: salt_bytes,
                    hash: hash_bytes,
                    created: parts[5].parse().unwrap_or(0),
                    attempts: parts[6].parse().unwrap_or(0),
                    last_send: parts[7].parse().unwrap_or(0),
                },
            );
        } else if parts.first() == Some(&"TICKET") && parts.len() == 6 {
            if parts[1].is_empty() || parts[2].is_empty() || parts[4].len() != 64 {
                continue;
            }
            store.tickets.insert(
                parts[4].to_string(),
                TicketRecord {
                    email: parts[1].to_string(),
                    host_id: parts[2].to_string(),
                    fingerprint: parts[3].to_string(),
                    created: parts[5].parse().unwrap_or(0),
                },
            );
        } else if parts.first() == Some(&"HOST") && parts.len() == 9 {
            if parts[1].is_empty() || parts[2].is_empty() || parts[6].is_empty() {
                continue;
            }
            store.hosts.insert(
                parts[2].to_string(),
                HostRecord {
                    email: parts[1].to_string(),
                    host_id: parts[2].to_string(),
                    name: parts[3].to_string(),
                    membership_id: parts[4].to_string(),
                    auth_epoch: parts[5].parse().unwrap_or(1).max(1),
                    machine_token: parts[6].to_string(),
                    device_public_key: parts[7].to_string(),
                    created: parts[8].parse().unwrap_or(0),
                },
            );
        } else if parts.first() == Some(&"SESS") && parts.len() == 4 {
            store.sessions.insert(
                parts[2].to_string(),
                SessionRecord {
                    email: parts[1].to_string(),
                    created: parts[3].parse().unwrap_or(0),
                },
            );
        }
    }
    Ok(())
}

impl Store {
    /** Reloads from disk. Used only at startup. */
    pub fn load(&mut self) -> Result<(), String> {
        load_rows(self)
    }

    /** Writes every row to a temp file and renames it. The plaintext code is not a column. */
    pub fn save(&self) -> Result<(), String> {
        let mut body = String::new();
        for row in self.codes.values() {
            body.push_str(&format!(
                "CODE\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                row.challenge_id,
                row.email,
                hex_encode(&row.salt),
                hex_encode(&row.hash),
                row.created,
                row.attempts,
                row.last_send
            ));
        }
        for (token, row) in &self.sessions {
            body.push_str(&format!("SESS\t{}\t{}\t{}\n", row.email, token, row.created));
        }
        for (hash_hex, row) in &self.tickets {
            body.push_str(&format!(
                "TICKET\t{}\t{}\t{}\t{}\t{}\n",
                row.email, row.host_id, row.fingerprint, hash_hex, row.created
            ));
        }
        for row in self.hosts.values() {
            body.push_str(&format!(
                "HOST\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                row.email,
                row.host_id,
                row.name,
                row.membership_id,
                row.auth_epoch,
                row.machine_token,
                row.device_public_key,
                row.created
            ));
        }
        let tmp = self.path.with_extension("tsv.tmp");
        fs::write(&tmp, body).map_err(|error| error.to_string())?;
        fs::rename(&tmp, &self.path).map_err(|error| error.to_string())
    }

    /** True when this code is still inside the 10 minute window. */
    pub fn code_live(row: &CodeRecord, now: u64) -> bool {
        row.created + CODE_TTL_SECS > now
    }

    /** Finds the email key for a challenge id. None when this process has no such row. */
    pub fn email_for_challenge(&self, challenge_id: &str) -> Option<String> {
        self.codes.iter().find(|(_, row)| row.challenge_id == challenge_id).map(|(email, _)| email.clone())
    }
}

extern "C" {
    fn flock(fd: i32, operation: i32) -> i32;
}
