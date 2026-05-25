//! USB enumeration and the CameraInfo handle (owns the nusb DeviceInfo so we
//! can re-open it later without re-enumerating).

use crate::Result;

pub const CANON_VENDOR_ID: u16 = 0x04A9;

#[derive(Debug, Clone)]
pub struct CameraInfo {
    pub(crate) inner: nusb::DeviceInfo,
}

impl CameraInfo {
    pub fn vendor_id(&self) -> u16 {
        self.inner.vendor_id()
    }
    pub fn product_id(&self) -> u16 {
        self.inner.product_id()
    }
    pub fn bus_number(&self) -> u8 {
        self.inner.bus_number()
    }
    pub fn device_address(&self) -> u8 {
        self.inner.device_address()
    }
    pub fn manufacturer(&self) -> Option<&str> {
        self.inner.manufacturer_string()
    }
    pub fn product(&self) -> Option<&str> {
        self.inner.product_string()
    }
    pub fn serial(&self) -> Option<&str> {
        self.inner.serial_number()
    }

    /// Open a PTP session against this camera.
    pub async fn open_ptp(&self) -> Result<crate::PtpSession> {
        crate::PtpSession::open(&self.inner).await
    }
}

/// Enumerate Canon devices visible on USB.
pub fn list_cameras() -> Result<Vec<CameraInfo>> {
    let cams = nusb::list_devices()?
        .filter(|d| d.vendor_id() == CANON_VENDOR_ID)
        .map(|inner| CameraInfo { inner })
        .collect();
    Ok(cams)
}
