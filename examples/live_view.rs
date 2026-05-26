//! Capture N live-view frames from the camera and save each as a PPM file.
//!
//! Usage: `cargo run --example live_view -- [count] [output_dir]`
//! Defaults: 5 frames to /tmp.
//!
//! PPM (P6) is a no-deps binary format any image viewer can open. Run with
//! e.g. `cargo run --example live_view -- 3 /tmp` to capture three frames.

use chdkptp::chdk::liveview::LV_TFR_VIEWPORT;
use chdkptp::{list_cameras, Error, Result};
use pollster::block_on;
use std::io::Write;
use std::time::Instant;

fn main() -> Result<()> {
    let count: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(5);
    let out_dir = std::env::args().nth(2).unwrap_or_else(|| "/tmp".into());

    let cam = list_cameras()?
        .into_iter()
        .next()
        .ok_or(Error::NoDevicesFound)?;

    println!(
        "Capturing {count} frames from {} {} (serial: {})",
        cam.manufacturer().unwrap_or("?"),
        cam.product().unwrap_or("?"),
        cam.serial().unwrap_or("?"),
    );

    block_on(async {
        let mut s = cam.open_ptp().await?;

        // The camera must be in record mode to produce a meaningful viewport.
        // (In playback mode it returns whatever the playback UI is showing.)
        let _ = s
            .execute_script_wait(
                "if not get_mode() then switch_mode_usb(1) sleep(3500) end",
                10_000,
            )
            .await?;

        for i in 0..count {
            let t = Instant::now();
            let frame = s.get_display_data(LV_TFR_VIEWPORT).await?;
            let dt_pull = t.elapsed();

            let t2 = Instant::now();
            let (width, height, rgb) = frame.decode_viewport_rgb()?;
            let dt_decode = t2.elapsed();

            let path = format!("{out_dir}/lv_{i:03}.ppm");
            save_ppm(&path, width, height, &rgb)
                .map_err(|e| Error::Usb(format!("save {path}: {e}")))?;

            println!(
                "frame {i:>2}: {width}x{height}  ver={}.{}  fb_type={}  \
                 pull={:>6.1}ms  decode={:>5.1}ms  → {path}",
                frame.header.version_major,
                frame.header.version_minor,
                frame.viewport_desc()?.map(|d| d.fb_type).unwrap_or(0),
                dt_pull.as_secs_f64() * 1000.0,
                dt_decode.as_secs_f64() * 1000.0,
            );
        }

        s.close().await?;
        Ok::<_, Error>(())
    })
}

fn save_ppm(path: &str, width: u32, height: u32, rgb: &[u8]) -> std::io::Result<()> {
    let mut f = std::fs::File::create(path)?;
    writeln!(f, "P6")?;
    writeln!(f, "{width} {height}")?;
    writeln!(f, "255")?;
    f.write_all(rgb)?;
    Ok(())
}
