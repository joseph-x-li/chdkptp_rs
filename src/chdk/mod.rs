//! CHDK PTP extensions.
//!
//! Every CHDK operation rides on a single vendor opcode (`PTP_OC_CHDK = 0x9999`)
//! with the sub-command in Param1. This module defines the sub-commands and
//! adds typed methods to `PtpSession` (in `ext`).

pub mod ext;
pub mod opcode;
pub mod script;

pub use ext::ChdkVersion;
pub use opcode::{Sub, PTP_OC_CHDK};
pub use script::{ErrorCategory, ScriptId, ScriptMsg, ScriptStatus, ScriptValue};
