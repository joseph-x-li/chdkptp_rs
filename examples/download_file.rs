//! Download a file from the camera's SD card.
//!
//! Usage: `cargo run --example download_file -- <camera_path> <local_path>`
//!
//! Example: `cargo run --example download_file -- A/DCIM/100CANON/IMG_0001.JPG out.jpg`

use chdkptp::{list_cameras, Error, Result};
use pollster::block_on;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage: {} <camera_path> <local_path>", args[0]);
        eprintln!("Camera path is rooted at A/  (the SD card).");
        std::process::exit(1);
    }
    let camera = &args[1];
    let local = &args[2];

    let cam = list_cameras()?
        .into_iter()
        .next()
        .ok_or(Error::NoDevicesFound)?;

    block_on(async {
        let mut s = cam.open_ptp().await?;
        println!("Downloading {camera} -> {local}");
        let t = std::time::Instant::now();
        let bytes = s.download_file(camera).await?;
        let dt = t.elapsed().as_secs_f64();
        let n = bytes.len();
        std::fs::write(local, &bytes).map_err(|e| Error::Usb(format!("write {local}: {e}")))?;
        let mbps = n as f64 / dt / 1_048_576.0;
        println!("Done: {n} bytes in {dt:.2}s ({mbps:.2} MB/s)");
        s.close().await?;
        Ok::<_, Error>(())
    })
}
