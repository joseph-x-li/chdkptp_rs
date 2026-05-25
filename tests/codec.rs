//! Wire-format unit tests for the PTP codec and container framing.

use chdkptp::ptp::codec::Reader;
use chdkptp::ptp::container::{decode, encode};
use chdkptp::ptp::opcode;

#[test]
fn container_command_roundtrip() {
    // Command container with two params (a CHDK ExecuteScript-style call).
    let bytes = encode(opcode::CONTAINER_COMMAND, 0x9999, 42, &[7, 0], &[]);

    // 12-byte header + 2 params * 4 bytes = 20 bytes
    assert_eq!(bytes.len(), 20);
    // Length-prefix should equal the total
    assert_eq!(&bytes[0..4], &20u32.to_le_bytes());

    let dec = decode(&bytes).expect("decode command");
    assert_eq!(dec.container_type, opcode::CONTAINER_COMMAND);
    assert_eq!(dec.code, 0x9999);
    assert_eq!(dec.txn_id, 42);
    assert_eq!(dec.params, vec![7, 0]);
    assert!(dec.payload.is_empty());
}

#[test]
fn container_data_roundtrip() {
    let payload = b"return get_zoom()";
    let bytes = encode(opcode::CONTAINER_DATA, 0x9999, 7, &[], payload);

    assert_eq!(bytes.len(), 12 + payload.len());

    let dec = decode(&bytes).expect("decode data");
    assert_eq!(dec.container_type, opcode::CONTAINER_DATA);
    assert_eq!(dec.code, 0x9999);
    assert_eq!(dec.txn_id, 7);
    assert!(dec.params.is_empty());
    assert_eq!(dec.payload, payload);
}

#[test]
fn container_decode_rejects_undersized_buffer() {
    let bytes = [1u8, 2, 3]; // shorter than 12-byte header
    assert!(decode(&bytes).is_err());
}

#[test]
fn container_decode_rejects_length_exceeding_buffer() {
    // Header claims length 1000 but we only provided 12 bytes.
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&1000u32.to_le_bytes());
    bytes.extend_from_slice(&[0; 8]);
    assert!(decode(&bytes).is_err());
}

#[test]
fn codec_read_string_utf16le() {
    // "Hi" is 2 chars + NUL = 3 chars including terminator.
    // Wire bytes: count=3, then 3 * 2 little-endian UTF-16 code units, last is NUL.
    let bytes = [3, 0x48, 0x00, 0x69, 0x00, 0x00, 0x00];
    let mut r = Reader::new(&bytes);
    assert_eq!(r.read_string().unwrap(), "Hi");
}

#[test]
fn codec_read_string_empty() {
    // A count of 0 means an empty string with no trailing NUL.
    let bytes = [0u8];
    let mut r = Reader::new(&bytes);
    assert_eq!(r.read_string().unwrap(), "");
}

#[test]
fn codec_read_u16_array() {
    // u32 count + N * u16, all little-endian.
    let bytes = [
        3, 0, 0, 0, // count = 3
        0x01, 0x10, // 0x1001
        0x02, 0x10, // 0x1002
        0x99, 0x99, // 0x9999
    ];
    let mut r = Reader::new(&bytes);
    let arr = r.read_u16_array().unwrap();
    assert_eq!(arr, vec![0x1001, 0x1002, 0x9999]);
}

#[test]
fn codec_primitive_reads_are_little_endian() {
    let bytes = [
        0x12, // u8
        0x34, 0x12, // u16
        0x78, 0x56, 0x34, 0x12, // u32
    ];
    let mut r = Reader::new(&bytes);
    assert_eq!(r.read_u8().unwrap(), 0x12);
    assert_eq!(r.read_u16().unwrap(), 0x1234);
    assert_eq!(r.read_u32().unwrap(), 0x1234_5678);
}
