//! chdkptp — pure-Rust client for Canon cameras running CHDK firmware.
//!
//! See `examples/` for end-to-end usage.

pub mod chdk;
pub mod error;
pub mod ptp;
pub mod usb;

pub use chdk::ChdkVersion;
pub use error::{Error, Result};
pub use ptp::{DeviceInfo, PtpSession};
pub use usb::{list_cameras, CameraInfo, CANON_VENDOR_ID};
