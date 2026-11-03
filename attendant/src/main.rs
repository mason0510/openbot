//! Attendant account API.
//!
//! Algorithm: OneTimeEmailCode.
//! It guarantees one code belongs to one normalized email, expires after 10 minutes, and verifies once.
//! Input is the email on `POST /v1/auth/email/start`. Verify takes that `challengeId` and the code.
//! It stops when verify succeeds, the code expires, or five attempts fail.
//! It does not guarantee that the recipient mail server accepts the message.
//! The store is `/srv/attendant/data/accounts.tsv`.

mod auth;
mod crypto;
mod dns;
mod http;
mod jwt;
mod mail;
mod signal;
mod store;

use std::env;
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use store::open_store;

/** Returns unix seconds from the machine clock. */
fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|value| value.as_secs()).unwrap_or(0)
}

fn main() {
    let bind = env::var("ATTENDANT_BIND").unwrap_or_else(|_| "127.0.0.1:8791".to_string());
    let data = env::var("ATTENDANT_DATA").unwrap_or_else(|_| "/srv/attendant/data".to_string());
    let from = env::var("ATTENDANT_FROM").unwrap_or_else(|_| "noreply@attendant.mwcpool.uk".to_string());
    let signal_url = env::var("ATTENDANT_SIGNAL_URL").unwrap_or_else(|_| "wss://api-attendant.mwcpool.uk/signal".to_string());
    let data_path = PathBuf::from(&data);
    let store = Arc::new(Mutex::new(open_store(&data_path).expect("open account store")));
    let registry = Arc::new(signal::Registry::new());
    let signer = Arc::new(jwt::TicketSigner::open(&data_path).expect("open ticket signer"));
    let listener = TcpListener::bind(&bind).expect("bind");
    eprintln!("attendant-api listening on {bind}");
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let store = Arc::clone(&store);
        let from = from.clone();
        let signal_url = signal_url.clone();
        let registry = Arc::clone(&registry);
        let signer = Arc::clone(&signer);
        thread::spawn(move || {
            let now = now_secs();
            if let Err(error) = http::handle(stream, &store, &from, &signal_url, &registry, &signer, now) {
                eprintln!("request failed: {error}");
            }
        });
    }
}
