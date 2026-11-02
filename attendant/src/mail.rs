use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use crate::crypto::base64;
use crate::dns::{lookup_a, lookup_mx};

/** Sends the sign-in code to the recipient MX on port 25. A failure sends nothing and stores nothing. */
pub fn send_code(from: &str, to: &str, code: &str) -> Result<(), String> {
    let domain = to.split_once('@').map(|(_, domain)| domain).ok_or_else(|| "invalid_email".to_string())?;
    let exchange = lookup_mx(domain)?;
    let address = lookup_a(&exchange)?;
    let mut stream = TcpStream::connect_timeout(&SocketAddr::new(address, 25), Duration::from_secs(10))
        .map_err(|error| error.to_string())?;
    stream.set_read_timeout(Some(Duration::from_secs(10))).ok();
    stream.set_write_timeout(Some(Duration::from_secs(10))).ok();
    expect_smtp(&mut stream, 220)?;
    smtp_cmd(&mut stream, "EHLO attendant.mwcpool.uk\r\n", 250)?;
    smtp_cmd(&mut stream, &format!("MAIL FROM:<{from}>\r\n"), 250)?;
    smtp_cmd(&mut stream, &format!("RCPT TO:<{to}>\r\n"), 250)?;
    smtp_cmd(&mut stream, "DATA\r\n", 354)?;
    let message = format!(
        "From: Attendant <{from}>\r\nTo: <{to}>\r\nSubject: =?utf-8?B?{}?=\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n登录验证码是 {code}\r\n10 分钟内有效，只能使用一次。\r\n.\r\n",
        base64("Attendant 登录验证码")
    );
    stream.write_all(message.as_bytes()).map_err(|error| error.to_string())?;
    expect_smtp(&mut stream, 250)?;
    let _ = stream.write_all(b"QUIT\r\n");
    Ok(())
}

/** Writes one SMTP command and requires the given reply code. */
fn smtp_cmd(stream: &mut TcpStream, command: &str, expect: u16) -> Result<(), String> {
    stream.write_all(command.as_bytes()).map_err(|error| error.to_string())?;
    expect_smtp(stream, expect)
}

/** Reads one SMTP reply. A multiline reply ends when the fourth character is a space. */
fn expect_smtp(stream: &mut TcpStream, expect: u16) -> Result<(), String> {
    let mut collected = String::new();
    let mut buffer = [0_u8; 1024];
    loop {
        let count = stream.read(&mut buffer).map_err(|error| error.to_string())?;
        if count == 0 {
            return Err(format!("smtp closed: {collected}"));
        }
        collected.push_str(&String::from_utf8_lossy(&buffer[..count]));
        if let Some(line) = collected.lines().last() {
            if line.len() >= 4 && line.as_bytes()[3] == b' ' {
                let code: u16 = line[..3].parse().unwrap_or(0);
                if code == expect {
                    return Ok(());
                }
                return Err(format!("smtp {code}"));
            }
        }
    }
}
