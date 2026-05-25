# chdkptp

Pure-Rust client for Canon cameras running [CHDK](https://chdk.fandom.com/) firmware. Talks PTP/USB directly via [`nusb`](https://crates.io/crates/nusb) — no `libusb` C dependency, no host-side Lua interpreter.

## Status

Early. The protocol surface is small and stable, but only a subset is implemented and only one camera model has been tested on the hardware bench.

| Capability | Status |
|---|---|
| USB enumeration | done |
| PTP container framing + codec + sessions | done |
| `GetDeviceInfo` parser | done |
| CHDK opcode dispatch + `chdk_version()` | done |
| Script execution + typed message decode (`shoot()`, raw Lua) | done |
| Synchronized multi-camera shoot (barrier + clock-sync) | done (examples) |
| File upload / download | not yet |
| Live view (YUV decode) | not yet |
| Remote-capture (USB-direct stills) | not yet |

## The Lua caveat

The original `chdkptp` tool's UX is built around host-side Lua, which is what this crate replaces — you write Rust instead.

**However:** CHDK firmware itself embeds a Lua interpreter that runs *on the camera*, and any non-trivial camera operation requires composing Lua source and shipping it over PTP via `ExecuteScript`. The crate hides this behind typed APIs (`session.shoot()`, etc.), but if you want arbitrary camera control you write small Lua snippets that run inside the camera. There is no way around this — CHDK has no other scripting interface.

Raw access for power users:

```rust
let _id = session.execute_script_lua("return get_zoom()").await?;
while let msg = session.read_script_msg().await? {
    // ScriptMsg::Return / Error / User / None
}
```

## Tested hardware

- Canon **PowerShot ELPH 180** (firmware 1.15.0.1.0, CHDK 2.9)

Other CHDK-supported PowerShots *should* work — the PTP wire format is identical across CHDK builds — but model-specific Lua quirks (button-press timing, available propcase numbers, available functions) exist and can surprise you. Bug reports with model + firmware + CHDK version welcome.

## Tested platforms

- macOS (Darwin 25). Linux and Windows should work because `nusb` is pure-Rust with platform backends for all three, but they are unverified.

## Quickstart

```rust
use chdkptp::{list_cameras, Error, Result};
use pollster::block_on;

fn main() -> Result<()> {
    let cam = list_cameras()?
        .into_iter()
        .next()
        .ok_or(Error::NoDevicesFound)?;

    block_on(async {
        let mut s = cam.open_ptp().await?;
        let info = s.get_device_info().await?;
        println!("{} (CHDK {})", info.model, s.chdk_version().await?);
        s.shoot().await?;  // takes a picture (auto-switches to record mode)
        s.close().await?;
        Ok::<_, Error>(())
    })
}
```

## Examples

Run with `cargo run --example <name>`:

| Example | What it does |
|---|---|
| `list_cameras` | Enumerate Canon devices on USB |
| `device_info` | Open a PTP session, print device descriptor + supported ops |
| `chdk_version` | Query CHDK PTP protocol version |
| `shoot` | Take a picture (auto-switches to record mode) |
| `exec_lua` | Run arbitrary Lua: `cargo run --example exec_lua -- 'return get_zoom()'` |
| `shoot_all` | Synchronized shoot across all connected cameras (barrier-based, ~10 ms host sync) |
| `shoot_all_clocksync` | Same, via per-camera tick-offset measurement (~3 ms inter-camera precision) |

## Architecture

Single crate. Internal modules: `usb`, `ptp/` (codec/container/session/device_info), `chdk/` (opcodes, script messaging, extension methods on `PtpSession`). Built on `nusb` (pure-Rust USB) and `thiserror`. Async-first; examples wrap with `pollster::block_on`.

## Limitations

- **Camera USB sleep.** Canon PowerShots drop off the USB bus after ~30 s of idle PTP traffic. The crate surfaces this as `Error::NoDevicesFound` on the next enumeration. Long-running tools should plan for re-enumeration; setting "Override Auto Power Down" in the CHDK menu is the cleanest in-camera fix.
- **Camera-side Lua loader quirk.** `ExecuteScript` source must be NUL-terminated (CHDK uses `luaL_loadstring`, not `luaL_loadbuffer`). The crate does this for you.
- **Per-model button-event timing.** The low-level `press('shoot_full')` works on some camera firmwares and not others. The high-level CHDK `shoot()` Lua function is the portable choice; see the `shoot()` helper.

## License

MIT — see [LICENSE](LICENSE).
