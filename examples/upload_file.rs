//! Upload a local file to the camera's SD card.
//!
//! Usage: `cargo run --example upload_file -- <local_path> <camera_path>`
//!
//! Example: `cargo run --example upload_file -- script.lua A/CHDK/SCRIPTS/script.lua`

use chdkptp::{list_cameras, Error, Result};
use pollster::block_on;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage: {} <local_path> <camera_path>", args[0]);
        eprintln!("Camera path is rooted at A/  (the SD card).");
        std::process::exit(1);
    }
    let local = &args[1];
    let camera = &args[2];

    let contents = std::fs::read(local).map_err(|e| Error::Usb(format!("read {local}: {e}")))?;

    let cam = list_cameras()?
        .into_iter()
        .next()
        .ok_or(Error::NoDevicesFound)?;

    block_on(async {
        let mut s = cam.open_ptp().await?;
        let bytes = contents.len();
        println!("Uploading {bytes} bytes: {local} -> {camera}");
        let t = std::time::Instant::now();
        s.upload_file(camera, &contents).await?;
        let dt = t.elapsed().as_secs_f64();
        let mbps = bytes as f64 / dt / 1_048_576.0;
        println!("Done in {dt:.2}s ({mbps:.2} MB/s)");
        s.close().await?;
        Ok::<_, Error>(())
    })
}
