//! Phase 1 end-to-end test: open a PTP session, GetDeviceInfo, print the salient fields.

use chdkptp::{list_cameras, Error, Result};
use pollster::block_on;

fn main() -> Result<()> {
    let cams = list_cameras()?;
    let cam = cams.into_iter().next().ok_or(Error::NoDevicesFound)?;

    println!(
        "Opening {} {} (serial: {})",
        cam.manufacturer().unwrap_or("?"),
        cam.product().unwrap_or("?"),
        cam.serial().unwrap_or("?"),
    );

    block_on(async {
        let mut session = cam.open_ptp().await?;
        let info = session.get_device_info().await?;

        println!();
        println!("Manufacturer:        {}", info.manufacturer);
        println!("Model:               {}", info.model);
        println!("Device version:      {}", info.device_version);
        println!("Serial:              {}", info.serial_number);
        println!("PTP standard:        0x{:04X}", info.standard_version);
        println!("Vendor extension ID: 0x{:08X}", info.vendor_extension_id);
        println!("Vendor extension:    {}", info.vendor_extension_desc);
        println!("Functional mode:     0x{:04X}", info.functional_mode);
        println!(
            "Operations supported ({}): {}",
            info.operations_supported.len(),
            format_ops(&info.operations_supported)
        );
        println!(
            "Events supported ({}): {}",
            info.events_supported.len(),
            format_ops(&info.events_supported)
        );
        println!(
            "Image formats ({}): {}",
            info.image_formats.len(),
            format_ops(&info.image_formats)
        );
        println!();
        if info.has_chdk() {
            println!("✓ CHDK vendor opcode 0x9999 is present in OperationsSupported.");
        } else {
            println!("✗ CHDK vendor opcode 0x9999 is NOT advertised. Is CHDK booted?");
        }

        session.close().await?;
        Ok::<_, chdkptp::Error>(())
    })?;

    Ok(())
}

fn format_ops(ops: &[u16]) -> String {
    let mut s = String::new();
    for (i, op) in ops.iter().enumerate() {
        if i > 0 {
            s.push_str(", ");
        }
        s.push_str(&format!("0x{:04X}", op));
    }
    s
}
