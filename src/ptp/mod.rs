//! PTP (Picture Transfer Protocol, PIMA 15740 / ISO 15740) layer.
//!
//! Provides:
//! - Wire codec (little-endian primitives, length-prefixed UTF-16LE strings, arrays)
//! - Container framing (Command / Data / Response)
//! - `PtpSession` driving bulk-in/bulk-out endpoints
//! - `DeviceInfo` parser

pub mod codec;
pub mod container;
pub mod device_info;
pub mod opcode;
pub mod session;

pub use device_info::DeviceInfo;
pub use session::{DataPhase, PtpResponse, PtpSession};
