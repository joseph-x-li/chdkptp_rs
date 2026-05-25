//! PTP container framing — the 12-byte header that prefixes every command,
//! data, response, and event block on the bulk endpoints.

use super::codec::{Reader, Writer};
use super::opcode;
use crate::{Error, Result};

pub const HEADER_LEN: usize = 12;

/// Encode a container ready to ship over bulk OUT.
///
/// For Command containers, `payload` is empty and `params` carries up to 5 u32s.
/// For Data containers, `params` is empty and `payload` is the raw bytes.
pub fn encode(
    container_type: u16,
    code: u16,
    txn_id: u32,
    params: &[u32],
    payload: &[u8],
) -> Vec<u8> {
    let body_len = params.len() * 4 + payload.len();
    let total = HEADER_LEN + body_len;

    let mut w = Writer::new();
    w.write_u32(total as u32);
    w.write_u16(container_type);
    w.write_u16(code);
    w.write_u32(txn_id);
    for p in params {
        w.write_u32(*p);
    }
    w.write_bytes(payload);
    w.into_bytes()
}

#[derive(Debug)]
pub struct Decoded {
    pub container_type: u16,
    pub code: u16,
    pub txn_id: u32,
    /// Populated for Command/Response containers (parsed from the body as u32s).
    pub params: Vec<u32>,
    /// Populated for Data containers (raw bytes after the header).
    pub payload: Vec<u8>,
}

/// Decode a complete container. The caller is responsible for having read
/// at least `length` bytes off the bulk-in endpoint.
pub fn decode(bytes: &[u8]) -> Result<Decoded> {
    if bytes.len() < HEADER_LEN {
        return Err(Error::Codec(format!(
            "container too short: {} bytes",
            bytes.len()
        )));
    }
    let mut r = Reader::new(bytes);
    let length = r.read_u32()? as usize;
    if length < HEADER_LEN {
        return Err(Error::Codec(format!(
            "container length {length} smaller than header"
        )));
    }
    if length > bytes.len() {
        return Err(Error::Codec(format!(
            "container length {length} exceeds buffer {}",
            bytes.len()
        )));
    }
    let container_type = r.read_u16()?;
    let code = r.read_u16()?;
    let txn_id = r.read_u32()?;

    let body = &bytes[HEADER_LEN..length];

    let (params, payload) = if container_type == opcode::CONTAINER_DATA {
        (Vec::new(), body.to_vec())
    } else {
        let n = body.len() / 4;
        let mut params = Vec::with_capacity(n);
        let mut br = Reader::new(body);
        for _ in 0..n {
            params.push(br.read_u32()?);
        }
        (params, Vec::new())
    };

    Ok(Decoded {
        container_type,
        code,
        txn_id,
        params,
        payload,
    })
}
