//! End-to-end: take a picture, derive the new file's path, download it.
//!
//! Canon names files `IMG_<NNNN>.JPG` where `NNNN` tracks `get_exp_count()`,
//! and `get_image_dir()` gives the current folder. So a freshly-taken shot
//! is reliably at `<get_image_dir()>/IMG_<get_exp_count()_padded_4>.JPG`.
//!
//! Usage: `cargo run --example shoot_and_download [<local_path>]`
//!         (default local path: ./latest.jpg)

use chdkptp::chdk::{ScriptMsg, ScriptValue};
use chdkptp::{list_cameras, Error, Result};
use pollster::block_on;
use std::time::Instant;

/// Run after `shoot()` to discover the just-taken file's full camera path.
const FIND_LATEST_LUA: &str = "\
    local d = get_image_dir() \
    local n = get_exp_count() \
    return string.format('%s/IMG_%04d.JPG', d, n)";

fn main() -> Result<()> {
    let local_path = std::env::args().nth(1).unwrap_or_else(|| "latest.jpg".into());

    let cam = list_cameras()?
        .into_iter()
        .next()
        .ok_or(Error::NoDevicesFound)?;

    block_on(async {
        let mut s = cam.open_ptp().await?;

        // Take the picture.
        let t = Instant::now();
        print!("Shooting… ");
        let shoot_msgs = s.shoot().await?;
        for m in &shoot_msgs {
            if let ScriptMsg::Error { text, .. } = m {
                eprintln!("\nshoot error: {text}");
                return Ok(());
            }
        }
        println!("done in {:.2}s", t.elapsed().as_secs_f64());

        // Discover the path of the new file.
        let lookup_msgs = s.execute_script_wait(FIND_LATEST_LUA, 5_000).await?;
        let camera_path = lookup_msgs
            .iter()
            .find_map(|m| match m {
                ScriptMsg::Return {
                    value: ScriptValue::String(p),
                    ..
                } => Some(p.clone()),
                _ => None,
            })
            .ok_or_else(|| Error::Usb("could not derive camera-side image path".into()))?;
        println!("Camera-side path: {camera_path}");

        // Download.
        let t = Instant::now();
        print!("Downloading… ");
        let bytes = s.download_file(&camera_path).await?;
        let dt = t.elapsed().as_secs_f64();
        let n = bytes.len();
        let mbps = n as f64 / dt / 1_048_576.0;
        println!("{n} bytes in {dt:.2}s ({mbps:.2} MB/s)");

        std::fs::write(&local_path, &bytes)
            .map_err(|e| Error::Usb(format!("write {local_path}: {e}")))?;
        println!("Saved -> {local_path}");

        s.close().await?;
        Ok::<_, Error>(())
    })
}
