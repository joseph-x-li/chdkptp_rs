//! Script messaging types: typed wrappers around the camera→host message protocol.

use super::opcode;

/// Identifier returned by `ExecuteScript`. Lets you tell messages from different
/// script invocations apart (rare, but the wire protocol carries it).
pub type ScriptId = u32;

/// Bitmask returned by `ScriptStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScriptStatus {
    raw: u32,
}

impl ScriptStatus {
    pub fn from_raw(raw: u32) -> Self {
        Self { raw }
    }
    pub fn raw(&self) -> u32 {
        self.raw
    }
    /// `true` if a script is currently executing on the camera.
    pub fn running(&self) -> bool {
        self.raw & opcode::SCRIPT_STATUS_RUN != 0
    }
    /// `true` if one or more messages are queued and ready to be drained.
    pub fn msg_pending(&self) -> bool {
        self.raw & opcode::SCRIPT_STATUS_MSG != 0
    }
}

/// Sub-category for `ScriptMsg::Error`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCategory {
    Compile,
    Runtime,
    Other(u32),
}

impl ErrorCategory {
    pub fn from_raw(v: u32) -> Self {
        match v {
            opcode::ERRTYPE_COMPILE => Self::Compile,
            opcode::ERRTYPE_RUN => Self::Runtime,
            _ => Self::Other(v),
        }
    }
}

/// A value carried by a `Return` or `User` script message.
#[derive(Debug, Clone, PartialEq)]
pub enum ScriptValue {
    Nil,
    Boolean(bool),
    Integer(i32),
    String(String),
    /// CHDK serializes tables to a Lua-source string on the camera side.
    Table(String),
    /// The camera reported a Lua type we don't support over the wire (function, userdata).
    Unsupported,
}

impl ScriptValue {
    /// Decode a value payload given the subtype tag.
    pub fn decode(subtype: u32, data: &[u8]) -> Self {
        match subtype {
            opcode::TYPE_NIL => ScriptValue::Nil,
            opcode::TYPE_BOOLEAN => {
                let b = data.first().copied().unwrap_or(0) != 0;
                ScriptValue::Boolean(b)
            }
            opcode::TYPE_INTEGER => {
                if data.len() >= 4 {
                    ScriptValue::Integer(i32::from_le_bytes([data[0], data[1], data[2], data[3]]))
                } else {
                    ScriptValue::Unsupported
                }
            }
            opcode::TYPE_STRING => ScriptValue::String(String::from_utf8_lossy(data).into_owned()),
            opcode::TYPE_TABLE => ScriptValue::Table(String::from_utf8_lossy(data).into_owned()),
            _ => ScriptValue::Unsupported,
        }
    }
}

/// One message drained from the script-messaging queue.
#[derive(Debug, Clone)]
pub enum ScriptMsg {
    /// The queue is empty. No more messages for now.
    None,
    /// The script raised an error. `category` distinguishes compile vs runtime;
    /// `text` is the human-readable message.
    Error {
        script_id: ScriptId,
        category: ErrorCategory,
        text: String,
    },
    /// The top-level chunk returned a value.
    Return {
        script_id: ScriptId,
        value: ScriptValue,
    },
    /// The script sent a user message via `write_usb_msg(...)`.
    User {
        script_id: ScriptId,
        value: ScriptValue,
    },
}
