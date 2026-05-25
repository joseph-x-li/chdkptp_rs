//! Phase 2 hello-world: ask the camera for its CHDK PTP protocol version.

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
        println!(
            "Model: {} (CHDK opcode advertised: {})",
            info.model,
            info.has_chdk()
        );

        let ver = session.chdk_version().await?;
        println!("CHDK PTP protocol version: {ver}");

        session.close().await?;
        Ok::<_, Error>(())
    })?;

    Ok(())
}
