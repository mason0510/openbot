use std::net::{IpAddr, Ipv4Addr, UdpSocket};
use std::time::Duration;

use crate::crypto::random_bytes;

struct DnsAnswer {
    type_code: u16,
    data: Vec<u8>,
    packet: Vec<u8>,
    offset: usize,
}

/** Resolves the preferred MX host. No MX record falls back to the domain itself. */
pub fn lookup_mx(domain: &str) -> Result<String, String> {
    let answers = dns_query(domain, 15)?;
    let mut best: Option<(u16, String)> = None;
    for answer in answers {
        if answer.type_code != 15 || answer.data.len() < 3 {
            continue;
        }
        let preference = u16::from_be_bytes([answer.data[0], answer.data[1]]);
        let exchange = parse_name(&answer.packet, answer.offset + 2)?;
        let replace = best.as_ref().map(|(current, _)| preference < *current).unwrap_or(true);
        if replace {
            best = Some((preference, exchange.trim_end_matches('.').to_string()));
        }
    }
    Ok(best.map(|(_, host)| host).unwrap_or_else(|| domain.to_string()))
}

/** Resolves the first IPv4 address of a host. */
pub fn lookup_a(host: &str) -> Result<IpAddr, String> {
    let answers = dns_query(host, 1)?;
    for answer in answers {
        if answer.type_code == 1 && answer.data.len() == 4 {
            return Ok(IpAddr::V4(Ipv4Addr::new(answer.data[0], answer.data[1], answer.data[2], answer.data[3])));
        }
    }
    Err(format!("no address for {host}"))
}

/** Sends one DNS query to 1.1.1.1 and waits at most 5 seconds. */
fn dns_query(name: &str, type_code: u16) -> Result<Vec<DnsAnswer>, String> {
    let mut packet = Vec::new();
    packet.extend_from_slice(&random_bytes(2)?);
    packet.extend_from_slice(&[0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    for label in name.trim_end_matches('.').split('.') {
        if label.len() > 63 || label.is_empty() {
            return Err("dns label".to_string());
        }
        packet.push(label.len() as u8);
        packet.extend_from_slice(label.as_bytes());
    }
    packet.extend_from_slice(&[0x00, (type_code >> 8) as u8, (type_code & 0xff) as u8, 0x00, 0x01]);
    let socket = UdpSocket::bind("0.0.0.0:0").map_err(|error| error.to_string())?;
    socket.set_read_timeout(Some(Duration::from_secs(5))).ok();
    socket.send_to(&packet, "1.1.1.1:53").map_err(|error| error.to_string())?;
    let mut response = [0_u8; 2048];
    let (count, _) = socket.recv_from(&mut response).map_err(|error| error.to_string())?;
    parse_answers(&response[..count])
}

/** Parses DNS answers. Name compression uses the original packet. */
fn parse_answers(packet: &[u8]) -> Result<Vec<DnsAnswer>, String> {
    if packet.len() < 12 {
        return Err("short dns".to_string());
    }
    let questions = u16::from_be_bytes([packet[4], packet[5]]);
    let answers = u16::from_be_bytes([packet[6], packet[7]]);
    let mut offset = 12_usize;
    for _ in 0..questions {
        parse_name_at(packet, &mut offset)?;
        offset += 4;
    }
    let mut parsed = Vec::new();
    for _ in 0..answers {
        parse_name_at(packet, &mut offset)?;
        if offset + 10 > packet.len() {
            return Err("short answer".to_string());
        }
        let type_code = u16::from_be_bytes([packet[offset], packet[offset + 1]]);
        let length = u16::from_be_bytes([packet[offset + 8], packet[offset + 9]]) as usize;
        offset += 10;
        if offset + length > packet.len() {
            return Err("short rdata".to_string());
        }
        parsed.push(DnsAnswer {
            type_code,
            data: packet[offset..offset + length].to_vec(),
            packet: packet.to_vec(),
            offset,
        });
        offset += length;
    }
    Ok(parsed)
}

/** Parses a DNS name and follows at most ten compression pointers. */
fn parse_name(packet: &[u8], start: usize) -> Result<String, String> {
    let mut offset = start;
    parse_name_at(packet, &mut offset)
}

/** Parses a DNS name at the cursor and advances the cursor past an uncompressed name. */
fn parse_name_at(packet: &[u8], offset: &mut usize) -> Result<String, String> {
    let mut labels = Vec::new();
    let mut hops = 0_u8;
    let mut cursor = *offset;
    let mut advanced = false;
    loop {
        if cursor >= packet.len() {
            return Err("name overflow".to_string());
        }
        let length = packet[cursor];
        if length == 0 {
            if !advanced {
                *offset = cursor + 1;
            }
            break;
        }
        if length & 0xc0 == 0xc0 {
            if cursor + 1 >= packet.len() {
                return Err("bad pointer".to_string());
            }
            if !advanced {
                *offset = cursor + 2;
                advanced = true;
            }
            cursor = (((length as usize) & 0x3f) << 8) | packet[cursor + 1] as usize;
            hops += 1;
            if hops > 10 {
                return Err("name loop".to_string());
            }
            continue;
        }
        cursor += 1;
        if cursor + length as usize > packet.len() {
            return Err("label overflow".to_string());
        }
        labels.push(String::from_utf8_lossy(&packet[cursor..cursor + length as usize]).to_string());
        cursor += length as usize;
        if !advanced {
            *offset = cursor;
        }
    }
    Ok(labels.join("."))
}
