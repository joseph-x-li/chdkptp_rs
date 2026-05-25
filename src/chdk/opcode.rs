//! CHDK vendor opcode + sub-command codes + script-message tags.
//!
//! Sub-command numeric values come from `chdk_headers/core/ptp.h` in the CHDK source.

/// The CHDK vendor operation code. All CHDK ops are multiplexed through this opcode.
pub const PTP_OC_CHDK: u16 = 0x9999;

/// Sub-command identifier, placed in `Param1` of the CHDK command container.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sub {
    Version = 0,
    GetMemory = 1,
    SetMemory = 2,
    CallFunction = 3,
    TempData = 4,
    UploadFile = 5,
    DownloadFile = 6,
    ExecuteScript = 7,
    ScriptStatus = 8,
    ScriptSupport = 9,
    ReadScriptMsg = 10,
    WriteScriptMsg = 11,
    GetDisplayData = 12,
    RemoteCaptureIsReady = 13,
    RemoteCaptureGetData = 14,
}

impl Sub {
    #[inline]
    pub fn as_u32(self) -> u32 {
        self as u32
    }
}

// --- Language selector for ExecuteScript (low byte of Param2) ---

pub const SCRIPT_LANG_LUA: u8 = 0;
pub const SCRIPT_LANG_UBASIC: u8 = 1;

// --- ExecuteScript flags (high bits of Param2) ---

pub const SCRIPT_FLAG_NOKILL: u32 = 0x100;
pub const SCRIPT_FLAG_FLUSH_CAM_MSGS: u32 = 0x200;
pub const SCRIPT_FLAG_FLUSH_HOST_MSGS: u32 = 0x400;

// --- ScriptStatus bitmask (Param1 of response) ---

pub const SCRIPT_STATUS_RUN: u32 = 0x01;
pub const SCRIPT_STATUS_MSG: u32 = 0x02;

// --- ReadScriptMsg type tags (Param1 of response) ---

pub const MSGTYPE_NONE: u32 = 0;
pub const MSGTYPE_ERR: u32 = 1;
pub const MSGTYPE_RET: u32 = 2;
pub const MSGTYPE_USER: u32 = 3;

// --- ReadScriptMsg subtype tags (Param2) — for RET/USER, indicates the Lua type ---

pub const TYPE_UNSUPPORTED: u32 = 0;
pub const TYPE_NIL: u32 = 1;
pub const TYPE_BOOLEAN: u32 = 2;
pub const TYPE_INTEGER: u32 = 3;
pub const TYPE_STRING: u32 = 4;
pub const TYPE_TABLE: u32 = 5;

// --- ReadScriptMsg ERR subtype categories (Param2 for MSGTYPE_ERR) ---

pub const ERRTYPE_NONE: u32 = 0;
pub const ERRTYPE_COMPILE: u32 = 1;
pub const ERRTYPE_RUN: u32 = 2;
