//! Real TCP LDAP v3 (RFC 4511 ASN.1 BER Simple BindRequest/BindResponse + SearchRequest/SearchResultEntry)
//! client for enterprise directory authentication.

use std::{
    io::{Read, Write},
    net::{TcpStream, ToSocketAddrs},
    time::Duration,
};

use gitust_core::AuthError;

fn ber_tlv(tag: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    let len = payload.len();
    if len < 0x80 {
        out.push(len as u8);
    } else if len <= 0xff {
        out.push(0x81);
        out.push(len as u8);
    } else {
        out.push(0x82);
        out.push(((len >> 8) & 0xff) as u8);
        out.push((len & 0xff) as u8);
    }
    out.extend_from_slice(payload);
    out
}

fn ber_read_len(bytes: &[u8], pos: &mut usize) -> Option<usize> {
    let first = *bytes.get(*pos)?;
    *pos += 1;
    if (first & 0x80) == 0 {
        return Some(usize::from(first));
    }
    let count = usize::from(first & 0x7f);
    if count == 0 || count > 2 || *pos + count > bytes.len() {
        return None;
    }
    let mut len = 0usize;
    for _ in 0..count {
        len = (len << 8) | usize::from(bytes[*pos]);
        *pos += 1;
    }
    Some(len)
}

fn parse_ldap_bind_result_code(packet: &[u8]) -> Option<u8> {
    let mut pos = 0;
    if *packet.get(pos)? != 0x30 {
        return None;
    }
    pos += 1;
    let _seq_len = ber_read_len(packet, &mut pos)?;
    if *packet.get(pos)? != 0x02 {
        return None;
    }
    pos += 1;
    let id_len = ber_read_len(packet, &mut pos)?;
    pos += id_len;
    if *packet.get(pos)? != 0x61 {
        return None;
    }
    pos += 1;
    let _bind_len = ber_read_len(packet, &mut pos)?;
    if *packet.get(pos)? != 0x0a {
        return None;
    }
    pos += 1;
    let rc_len = ber_read_len(packet, &mut pos)?;
    if rc_len != 1 {
        return None;
    }
    packet.get(pos).copied()
}

fn ber_next_tlv<'a>(bytes: &'a [u8], pos: &mut usize) -> Option<(u8, &'a [u8])> {
    let tag = *bytes.get(*pos)?;
    *pos += 1;
    let len = ber_read_len(bytes, pos)?;
    let slice = bytes.get(*pos..*pos + len)?;
    *pos += len;
    Some((tag, slice))
}

fn parse_search_result_entry_attributes(packet: &[u8]) -> Option<(String, String)> {
    let mut pos = 0;
    let mut found_mail = None;
    let mut found_uid = None;
    while pos < packet.len() {
        let (msg_tag, msg_body) = ber_next_tlv(packet, &mut pos)?;
        if msg_tag != 0x30 {
            continue;
        }
        let mut mpos = 0;
        let (id_tag, _) = ber_next_tlv(msg_body, &mut mpos)?;
        if id_tag != 0x02 {
            continue;
        }
        let (op_tag, op_body) = ber_next_tlv(msg_body, &mut mpos)?;
        if op_tag != 0x64 {
            continue;
        }
        let mut epos = 0;
        let (dn_tag, _) = ber_next_tlv(op_body, &mut epos)?;
        if dn_tag != 0x04 {
            continue;
        }
        let (attrs_tag, attrs_body) = ber_next_tlv(op_body, &mut epos)?;
        if attrs_tag != 0x30 {
            continue;
        }
        let mut apos = 0;
        while apos < attrs_body.len() {
            let (pa_tag, pa_body) = ber_next_tlv(attrs_body, &mut apos)?;
            if pa_tag != 0x30 {
                continue;
            }
            let mut ppos = 0;
            let (type_tag, type_bytes) = ber_next_tlv(pa_body, &mut ppos)?;
            let (vals_tag, vals_body) = ber_next_tlv(pa_body, &mut ppos)?;
            if type_tag != 0x04 || vals_tag != 0x31 {
                continue;
            }
            let mut vpos = 0;
            let Some((val_tag, val_bytes)) = ber_next_tlv(vals_body, &mut vpos) else {
                continue;
            };
            if val_tag != 0x04 {
                continue;
            }
            // fail-open-ok: non-UTF8 binary LDAP attributes are skipped while extracting textual mail/uid attributes; missing email/username fails closed below.
            let (Ok(attr_name), Ok(attr_val)) = (
                std::str::from_utf8(type_bytes),
                std::str::from_utf8(val_bytes),
            ) else {
                continue;
            };
            let clean_val = attr_val.trim();
            if !clean_val.is_empty() {
                if attr_name.eq_ignore_ascii_case("mail") {
                    found_mail = Some(clean_val.to_ascii_lowercase());
                } else if attr_name.eq_ignore_ascii_case("uid") {
                    found_uid = Some(clean_val.to_owned());
                }
            }
        }
    }
    Some((found_mail?, found_uid?))
}

pub(crate) fn authenticate_ldap_v3_over_tcp(
    ldap_url: &str,
    bind_dn: &str,
    ldap_password: &str,
) -> Result<(String, String), AuthError> {
    let rest = ldap_url
        .trim()
        .strip_prefix("ldap://")
        .ok_or(AuthError::InvalidCredentials)?;
    let host_port = match rest.split_once('/') {
        Some((hp, _)) => hp,
        None => rest,
    };
    let addr = host_port
        .to_socket_addrs()
        .map_err(|_| AuthError::InvalidCredentials)?
        .next()
        .ok_or(AuthError::InvalidCredentials)?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(3))
        .map_err(|_| AuthError::InvalidCredentials)?;
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .map_err(|_| AuthError::InvalidCredentials)?;
    stream
        .set_write_timeout(Some(Duration::from_secs(3)))
        .map_err(|_| AuthError::InvalidCredentials)?;

    // 1. Send RFC 4511 LDAP v3 Simple BindRequest (0x60)
    let bind_req_body = [
        vec![0x02, 0x01, 0x03],
        ber_tlv(0x04, bind_dn.as_bytes()),
        ber_tlv(0x80, ldap_password.as_bytes()),
    ]
    .concat();
    let bind_msg = ber_tlv(
        0x30,
        &[vec![0x02, 0x01, 0x01], ber_tlv(0x60, &bind_req_body)].concat(),
    );
    stream
        .write_all(&bind_msg)
        .map_err(|_| AuthError::InvalidCredentials)?;

    let mut resp_buf = [0u8; 2048];
    let n = stream
        .read(&mut resp_buf)
        .map_err(|_| AuthError::InvalidCredentials)?;
    if parse_ldap_bind_result_code(&resp_buf[..n]) != Some(0) {
        return Err(AuthError::InvalidCredentials);
    }

    // 2. Send RFC 4511 LDAP v3 SearchRequest (0x63) for baseObject = bind_dn
    let attrs_req = ber_tlv(
        0x30,
        &[ber_tlv(0x04, b"mail"), ber_tlv(0x04, b"uid")].concat(),
    );
    let search_body = [
        ber_tlv(0x04, bind_dn.as_bytes()),
        vec![
            0x0a, 0x01, 0x00, // scope: baseObject (0)
            0x0a, 0x01, 0x00, // derefAliases: neverDerefAliases (0)
            0x02, 0x01, 0x01, // sizeLimit: 1
            0x02, 0x01, 0x05, // timeLimit: 5
            0x01, 0x01, 0x00, // typesOnly: FALSE
        ],
        ber_tlv(0x87, b"objectClass"),
        attrs_req,
    ]
    .concat();
    let search_msg = ber_tlv(
        0x30,
        &[vec![0x02, 0x01, 0x02], ber_tlv(0x63, &search_body)].concat(),
    );
    stream
        .write_all(&search_msg)
        .map_err(|_| AuthError::InvalidCredentials)?;

    let n2 = stream
        .read(&mut resp_buf)
        .map_err(|_| AuthError::InvalidCredentials)?;
    parse_search_result_entry_attributes(&resp_buf[..n2]).ok_or(AuthError::InvalidCredentials)
}
