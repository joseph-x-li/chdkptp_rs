use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("USB I/O: {0}")]
    Usb(String),

    #[error("no Canon devices found on USB")]
    NoDevicesFound,

    #[error("no PTP/still-image interface on device")]
    NoPtpInterface,

    #[error("PTP wire codec: {0}")]
    Codec(String),

    #[error("PTP responded with code 0x{code:04X}")]
    PtpResponse { code: u16 },

    #[error("unexpected PTP container type {got} (expected {expected})")]
    UnexpectedContainer { expected: u16, got: u16 },
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Usb(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
