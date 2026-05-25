//! Phase 0 sanity check: enumerate Canon USB devices.

use chdkptp::list_cameras;

fn main() -> chdkptp::Result<()> {
    let cams = list_cameras()?;

    if cams.is_empty() {
        println!("No Canon devices found.");
        return Ok(());
    }

    println!("Found {} Canon device(s):", cams.len());
    for c in &cams {
        println!(
            "  bus {:>3} addr {:>3}  VID=0x{:04X} PID=0x{:04X}  {} {}  serial={}",
            c.bus_number(),
            c.device_address(),
            c.vendor_id(),
            c.product_id(),
            c.manufacturer().unwrap_or("?"),
            c.product().unwrap_or("?"),
            c.serial().unwrap_or("?"),
        );
    }
    Ok(())
}
