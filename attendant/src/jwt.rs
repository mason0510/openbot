//! ES256 remote-session tickets.
//!
//! Algorithm: AttendantTicketSign.
//! It guarantees a client ticket is an ES256 JWT whose `aud` is `openbot-remote`, and that
//! `/.well-known/jwks.json` publishes the matching P-256 public key.
//! Input is the session claims and the PEM at `$ATTENDANT_DATA/remote-ticket.pem`.
//! Trigger: `POST /v2/remote/sessions/{id}/ticket`.
//! Stop: openssl cannot sign, or the PEM cannot be created.
//! It does not guarantee a phone completes ICE. It does not log the PEM or the ticket.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::crypto::{base64url, random_bytes};

const KID: &str = "attendant-1";
const OPENSSL: &str = "/usr/bin/openssl";

/** The signing key for client Signal tickets. The PEM stays on this machine. */
pub struct TicketSigner {
    key_path: PathBuf,
    jwks: String,
}

/** Claims the desktop reads from a client ticket and from `peer-ready`. Seconds, not milliseconds. */
#[derive(Clone)]
pub struct ClientTicketClaims {
    pub host_id: String,
    pub session_id: String,
    pub user_id: String,
    pub membership_id: String,
    pub role: String,
    pub auth_epoch: u64,
    pub session_expires_at: u64,
    pub client_public_key: String,
}

impl TicketSigner {
    /**
     * open: Loads or creates the P-256 PEM and caches the public JWKS.
     * Guarantees: `jwks()` is a one-key set whose `kid` is `attendant-1`.
     * Refusals: openssl missing, key generation failed, or the public point is not P-256.
     * Boundaries: does not sign a ticket. A new PEM is mode 0600.
     */
    pub fn open(data_dir: &Path) -> Result<Self, String> {
        let key_path = data_dir.join("remote-ticket.pem");
        if !key_path.exists() {
            create_key(&key_path)?;
        }
        let jwks = jwks_document(&key_path)?;
        Ok(Self { key_path, jwks })
    }

    /** The public JWKS body. It has no private key. */
    pub fn jwks(&self) -> &str {
        &self.jwks
    }

    /**
     * sign_client: Signs one client ticket.
     * Guarantees: the compact JWT verifies with this signer's public key, audience `openbot-remote`.
     * Refusals: openssl sign failure, or a claim that cannot sit in JSON.
     * Boundaries: does not register the ticket. Does not print the token.
     */
    pub fn sign_client(&self, claims: &ClientTicketClaims, now: u64) -> Result<String, String> {
        if claims.host_id.is_empty()
            || claims.session_id.is_empty()
            || claims.user_id.is_empty()
            || claims.membership_id.is_empty()
            || claims.role.is_empty()
            || claims.client_public_key.is_empty()
        {
            return Err("claim".to_string());
        }
        let jti = base64url(&random_bytes(16).map_err(|_| "random".to_string())?);
        let exp = now.saturating_add(86_400).min(claims.session_expires_at);
        if exp <= now {
            return Err("expired".to_string());
        }
        let header = format!(r#"{{"alg":"ES256","typ":"JWT","kid":"{KID}"}}"#);
        let payload = format!(
            r#"{{"aud":"openbot-remote","jti":"{jti}","sessionId":"{}","hostId":"{}","userId":"{}","membershipId":"{}","role":"{}","authEpoch":{},"protocolMinimum":2,"protocolMaximum":2,"sessionExpiresAt":{},"clientPublicKey":"{}","iat":{now},"exp":{exp}}}"#,
            json_escape(&claims.session_id),
            json_escape(&claims.host_id),
            json_escape(&claims.user_id),
            json_escape(&claims.membership_id),
            json_escape(&claims.role),
            claims.auth_epoch,
            claims.session_expires_at,
            json_escape(&claims.client_public_key),
        );
        let signing_input = format!("{}.{}", base64url(header.as_bytes()), base64url(payload.as_bytes()));
        let der = openssl_sign(&self.key_path, signing_input.as_bytes())?;
        let raw = der_ecdsa_to_raw(&der)?;
        Ok(format!("{signing_input}.{}", base64url(&raw)))
    }
}

/** Creates a P-256 PEM. Refuses when openssl cannot write the file. */
fn create_key(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let status = Command::new(OPENSSL)
        .args(["ecparam", "-name", "prime256v1", "-genkey", "-noout", "-out"])
        .arg(path)
        .status()
        .map_err(|error| error.to_string())?;
    if !status.success() {
        return Err("openssl genkey failed".to_string());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|error| error.to_string())?;
    }
    Ok(())
}

/** Builds the JWKS from the PEM public point. */
fn jwks_document(path: &Path) -> Result<String, String> {
    let der = Command::new(OPENSSL)
        .args(["pkey", "-pubout", "-outform", "DER", "-in"])
        .arg(path)
        .output()
        .map_err(|error| error.to_string())?;
    if !der.status.success() {
        return Err("openssl pubout failed".to_string());
    }
    let (x, y) = public_point(&der.stdout)?;
    Ok(format!(
        r#"{{"keys":[{{"kty":"EC","crv":"P-256","alg":"ES256","use":"sig","kid":"{KID}","x":"{}","y":"{}"}}]}}"#,
        base64url(&x),
        base64url(&y)
    ))
}

/** Last uncompressed P-256 point in an SPKI DER blob. */
fn public_point(der: &[u8]) -> Result<([u8; 32], [u8; 32]), String> {
    if der.len() < 65 {
        return Err("short public key".to_string());
    }
    let point = &der[der.len() - 65..];
    if point[0] != 0x04 {
        return Err("public key is not uncompressed P-256".to_string());
    }
    let mut x = [0_u8; 32];
    let mut y = [0_u8; 32];
    x.copy_from_slice(&point[1..33]);
    y.copy_from_slice(&point[33..65]);
    Ok((x, y))
}

/** Signs with openssl and returns the DER signature. The PEM path is not logged. */
fn openssl_sign(path: &Path, input: &[u8]) -> Result<Vec<u8>, String> {
    let mut child = Command::new(OPENSSL)
        .args(["dgst", "-sha256", "-sign"])
        .arg(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| error.to_string())?;
    child
        .stdin
        .take()
        .ok_or_else(|| "openssl stdin".to_string())?
        .write_all(input)
        .map_err(|error| error.to_string())?;
    let output = child.wait_with_output().map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err("openssl sign failed".to_string());
    }
    Ok(output.stdout)
}

/**
 * der_ecdsa_to_raw: Turns an openssl ECDSA DER signature into the 64-byte P-256 form jose expects.
 * Guarantees: high-bit integers keep their value after the DER `00` pad is removed.
 * Refusals: a signature that is not a SEQUENCE of two INTEGERs, or an integer longer than 32 bytes.
 * Boundaries: does not verify the signature.
 */
pub fn der_ecdsa_to_raw(der: &[u8]) -> Result<[u8; 64], String> {
    if der.first() != Some(&0x30) || der.len() < 8 {
        return Err("bad der".to_string());
    }
    let (header, len) = der_length(&der[1..])?;
    let seq_end = 1 + header + len;
    if der.len() < seq_end {
        return Err("short sequence".to_string());
    }
    let seq = &der[1 + header..seq_end];
    let (r, after_r) = der_integer(seq)?;
    let (s, _) = der_integer(&seq[after_r..])?;
    let mut raw = [0_u8; 64];
    copy_fixed(&mut raw[..32], &r)?;
    copy_fixed(&mut raw[32..], &s)?;
    Ok(raw)
}

/** `(header_len, value_len)` for one DER length. */
fn der_length(bytes: &[u8]) -> Result<(usize, usize), String> {
    let first = *bytes.first().ok_or_else(|| "short der".to_string())?;
    if first < 0x80 {
        return Ok((1, first as usize));
    }
    let count = (first & 0x7f) as usize;
    if count == 0 || count > 2 || bytes.len() < 1 + count {
        return Err("bad der length".to_string());
    }
    let mut len = 0_usize;
    for byte in &bytes[1..1 + count] {
        len = (len << 8) | *byte as usize;
    }
    Ok((1 + count, len))
}

/** One INTEGER, returned as the magnitude bytes and the end offset in `bytes`. */
fn der_integer(bytes: &[u8]) -> Result<(Vec<u8>, usize), String> {
    if bytes.first() != Some(&0x02) {
        return Err("bad integer".to_string());
    }
    let (header, len) = der_length(&bytes[1..])?;
    let start = 1 + header;
    let end = start + len;
    if bytes.len() < end || len == 0 {
        return Err("short integer".to_string());
    }
    Ok((bytes[start..end].to_vec(), end))
}

/** Copies a DER integer into a fixed big-endian field. A leading `00` pad is dropped. */
fn copy_fixed(out: &mut [u8], value: &[u8]) -> Result<(), String> {
    let magnitude = if value.len() > 1 && value[0] == 0 { &value[1..] } else { value };
    if magnitude.len() > out.len() {
        return Err("integer too wide".to_string());
    }
    let start = out.len() - magnitude.len();
    out[start..].copy_from_slice(magnitude);
    Ok(())
}

/** Escapes one claim for a JSON string. A PEM newline must stay a newline after the desktop parses it. */
fn json_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::der_ecdsa_to_raw;

    #[test]
    fn der_keeps_a_high_bit_integer() {
        // SEQUENCE { INTEGER 0x80, INTEGER 0x01 }
        let der = [0x30, 0x07, 0x02, 0x02, 0x00, 0x80, 0x02, 0x01, 0x01];
        let raw = der_ecdsa_to_raw(&der).expect("raw");
        assert_eq!(raw[31], 0x80);
        assert_eq!(raw[63], 0x01);
    }
}
