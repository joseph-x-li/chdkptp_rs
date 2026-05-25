//! Probe: send arbitrary Lua source and dump the full message transcript.
//!
//! Usage: `cargo run --example exec_lua -- 'return get_pic_count()'`

use chdkptp::chdk::{ScriptMsg, ScriptValue};
use chdkptp::{list_cameras, Error, Result};
use pollster::block_on;

fn main() -> Result<()> {
    let source = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "return 'hello from CHDK, zoom='..tostring(get_zoom())".to_string());

    let cams = list_cameras()?;
    let cam = cams.into_iter().next().ok_or(Error::NoDevicesFound)?;

    block_on(async {
        let mut session = cam.open_ptp().await?;
        println!("Lua: {source}");
        let msgs = session.execute_script_wait(&source, 10_000).await?;

        println!("{} message(s):", msgs.len());
        for m in &msgs {
            match m {
                ScriptMsg::Error { category, text, .. } => {
                    println!("  ! ERR [{category:?}]: {text}")
                }
                ScriptMsg::Return { value, .. } => println!("  ← RET: {}", fmt_value(value)),
                ScriptMsg::User { value, .. } => println!("  · USER: {}", fmt_value(value)),
                ScriptMsg::None => {}
            }
        }
        session.close().await?;
        Ok::<_, Error>(())
    })?;
    Ok(())
}

fn fmt_value(v: &ScriptValue) -> String {
    match v {
        ScriptValue::Nil => "nil".into(),
        ScriptValue::Boolean(b) => b.to_string(),
        ScriptValue::Integer(i) => i.to_string(),
        ScriptValue::String(s) => format!("{s:?}"),
        ScriptValue::Table(s) => format!("table {s}"),
        ScriptValue::Unsupported => "<unsupported>".into(),
    }
}
