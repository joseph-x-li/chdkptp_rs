//! Phase 3 hello-world: take a picture by sending `shoot()` to the camera.

use chdkptp::chdk::{ScriptMsg, ScriptValue};
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
        if !info.has_chdk() {
            eprintln!("CHDK not advertised — is the camera booted with CHDK?");
            return Ok(());
        }
        println!(
            "Model: {} (CHDK {})",
            info.model,
            session.chdk_version().await?
        );

        println!("Sending shoot()…");
        let msgs = session.shoot().await?;

        println!("Script finished, {} message(s):", msgs.len());
        for m in &msgs {
            print_msg(m);
        }

        // Summarize
        let errors: Vec<_> = msgs
            .iter()
            .filter(|m| matches!(m, ScriptMsg::Error { .. }))
            .collect();
        if errors.is_empty() {
            println!("✓ shoot() completed without errors.");
        } else {
            println!("✗ {} error message(s) — see above.", errors.len());
        }

        session.close().await?;
        Ok::<_, Error>(())
    })?;

    Ok(())
}

fn print_msg(m: &ScriptMsg) {
    match m {
        ScriptMsg::None => println!("  - (none)"),
        ScriptMsg::Error { category, text, .. } => {
            println!("  ! ERR [{category:?}]: {text}")
        }
        ScriptMsg::Return { value, .. } => match value {
            ScriptValue::Nil => println!("  ← RET: nil"),
            ScriptValue::Boolean(b) => println!("  ← RET: {b}"),
            ScriptValue::Integer(i) => println!("  ← RET: {i}"),
            ScriptValue::String(s) => println!("  ← RET: {s:?}"),
            ScriptValue::Table(s) => println!("  ← RET: table {s}"),
            ScriptValue::Unsupported => println!("  ← RET: <unsupported type>"),
        },
        ScriptMsg::User { value, .. } => println!("  · USER: {value:?}"),
    }
}
