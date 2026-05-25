//! Standard PTP container types, operation codes, and response codes.
//!
//! Only the subset we currently use is enumerated.

// Container types (PTP/USB)
pub const CONTAINER_COMMAND: u16 = 1;
pub const CONTAINER_DATA: u16 = 2;
pub const CONTAINER_RESPONSE: u16 = 3;
pub const CONTAINER_EVENT: u16 = 4;

// Standard operation codes
pub const OP_GET_DEVICE_INFO: u16 = 0x1001;
pub const OP_OPEN_SESSION: u16 = 0x1002;
pub const OP_CLOSE_SESSION: u16 = 0x1003;

// Standard response codes
pub const RC_OK: u16 = 0x2001;
pub const RC_GENERAL_ERROR: u16 = 0x2002;
pub const RC_SESSION_NOT_OPEN: u16 = 0x2003;
pub const RC_OPERATION_NOT_SUPPORTED: u16 = 0x2005;
pub const RC_SESSION_ALREADY_OPEN: u16 = 0x201E;

pub fn response_name(code: u16) -> &'static str {
    match code {
        RC_OK => "OK",
        RC_GENERAL_ERROR => "General_Error",
        RC_SESSION_NOT_OPEN => "Session_Not_Open",
        RC_OPERATION_NOT_SUPPORTED => "Operation_Not_Supported",
        RC_SESSION_ALREADY_OPEN => "Session_Already_Open",
        _ => "Unknown",
    }
}
