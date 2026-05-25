//! GetDeviceInfo response dataset parser.

use super::codec::Reader;
use crate::Result;

#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub standard_version: u16,
    pub vendor_extension_id: u32,
    pub vendor_extension_version: u16,
    pub vendor_extension_desc: String,
    pub functional_mode: u16,
    pub operations_supported: Vec<u16>,
    pub events_supported: Vec<u16>,
    pub device_properties_supported: Vec<u16>,
    pub capture_formats: Vec<u16>,
    pub image_formats: Vec<u16>,
    pub manufacturer: String,
    pub model: String,
    pub device_version: String,
    pub serial_number: String,
}

impl DeviceInfo {
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut r = Reader::new(bytes);
        Ok(Self {
            standard_version: r.read_u16()?,
            vendor_extension_id: r.read_u32()?,
            vendor_extension_version: r.read_u16()?,
            vendor_extension_desc: r.read_string()?,
            functional_mode: r.read_u16()?,
            operations_supported: r.read_u16_array()?,
            events_supported: r.read_u16_array()?,
            device_properties_supported: r.read_u16_array()?,
            capture_formats: r.read_u16_array()?,
            image_formats: r.read_u16_array()?,
            manufacturer: r.read_string()?,
            model: r.read_string()?,
            device_version: r.read_string()?,
            serial_number: r.read_string()?,
        })
    }

    /// `true` if the device exposes the CHDK vendor opcode 0x9999.
    pub fn has_chdk(&self) -> bool {
        self.operations_supported.contains(&0x9999)
    }
}
