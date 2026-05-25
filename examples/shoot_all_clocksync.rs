//! Synchronized multi-camera shoot via per-camera tick-offset measurement +
//! camera-side busy-wait.
//!
//! Each camera runs ONE combined Lua script (warmup → busy-wait → fire), so
//! the half-press stays held inside a single lua_State. (CHDK auto-releases
//! held keys when the lua_State is destroyed, so splitting WARMUP and FIRE
//! across two ExecuteScript calls would silently drop the half-press —
//! verified by probe.)
//!
//! Per camera, in its own host thread:
//!   1. Measure tick offset between host wall-clock and camera tick
//!      (NTP-style, multiple samples, pick best RTT).
//!   2. Sync at a barrier so the leader can pick a shared target wall-clock.
//!   3. Send the combined warmup+busy-wait+fire script with the per-camera
//!      target tick baked in.

use chdkptp::chdk::{ScriptMsg, ScriptValue};
use chdkptp::{list_cameras, Result};
use pollster::block_on;
use std::sync::{Arc, Barrier, Mutex};
use std::thread;
use std::time::Instant;

const N_OFFSET_SAMPLES: usize = 20;

/// How far in the future to schedule the synchronized shot.
/// Must exceed (slowest camera warmup) + safety margin. ELPH 180 warmup is
/// ~700 ms from record mode, ~3.5 s with cold mode-switch from playback.
/// 2.5 s keeps busy-wait short enough to avoid triggering Smart Shutter /
/// Hybrid Auto's "auto-fire on long half-press" behavior. Bump if you see
/// `busy-wait = 0ms` in the summary (warmup overran target).
const TARGET_LEAD_MS: f64 = 2500.0;

/// Probe script: just return the camera's current tick. Used for offset
/// measurement; no held state, so cross-script teardown doesn't matter.
const TICK_PROBE_LUA: &str = "return get_tick_count()";

/// The combined per-camera script: warmup → busy-wait → fire → return.
///
/// All in one lua_State so the half-press is genuinely held throughout.
///
/// Returns nine comma-separated camera-side values:
///   `<t_start>,<warmup_done>,<t_exit>,<t_done>,<exp_at_start>,<exp_after_mode>,<exp_after_half>,<exp_before_fire>,<exp_after>`
///
/// The five exp_count checkpoints let us pinpoint exactly when any unwanted
/// shutter actuation happens (mode switch, half-press, during busy-wait, etc).
fn combined_lua(target_tick: u64) -> String {
    format!(
        "local t_start = get_tick_count() \
         local exp_at_start = get_exp_count() \
         if not get_mode() then \
           switch_mode_usb(1) \
           sleep(3500) \
         end \
         local exp_after_mode = get_exp_count() \
         local ok, p = pcall(require, 'propcase') \
         if ok then \
           if p.FLASH_MODE then set_prop(p.FLASH_MODE, 2) end \
           if p.WB_MODE    then set_prop(p.WB_MODE, 1)    end \
           if p.DRIVE_MODE then set_prop(p.DRIVE_MODE, 0) end \
         end \
         if type(set_iso_mode)    == 'function' then set_iso_mode(1)     end \
         if type(set_sv96)        == 'function' then set_sv96(411)        end \
         if type(set_tv96_direct) == 'function' then set_tv96_direct(576) end \
         press('shoot_half') \
         local af_start = get_tick_count() \
         while not get_shooting() and (get_tick_count() - af_start) < 5000 do \
           sleep(50) \
         end \
         sleep(200) \
         local warmup_done = get_tick_count() \
         local exp_after_half = get_exp_count() \
         local target = {target_tick} \
         while get_tick_count() < target do end \
         local t_exit = get_tick_count() \
         local exp_before_fire = get_exp_count() \
         press('shoot_full') \
         sleep(150) \
         release('shoot_full') \
         release('shoot_half') \
         local t_done = get_tick_count() \
         sleep(1800) \
         local exp_after = get_exp_count() \
         return t_start..','..warmup_done..','..t_exit..','..t_done..','..exp_at_start..','..exp_after_mode..','..exp_after_half..','..exp_before_fire..','..exp_after"
    )
}

#[derive(Default, Debug)]
#[allow(dead_code)]
struct CamTiming {
    label: String,
    offset_ms: f64,
    offset_rtt_ms: f64,
    target_tick: u64,
    script_send_ms: f64,
    script_recv_ms: f64,
    cam_tick_start: Option<u64>,
    cam_tick_warmup_done: Option<u64>,
    cam_tick_exit: Option<u64>,
    cam_tick_done: Option<u64>,
    actual_exit_host_ms: Option<f64>,
    exp_at_start: Option<u64>,
    exp_after_mode: Option<u64>,
    exp_after_half: Option<u64>,
    exp_before_fire: Option<u64>,
    exp_after: Option<u64>,
    errors: Vec<String>,
}

fn main() -> Result<()> {
    let t0 = Instant::now();

    let cams = list_cameras()?;
    if cams.is_empty() {
        eprintln!("No Canon devices found.");
        return Ok(());
    }
    log(
        t0,
        &format!("list_cameras returned {} device(s)", cams.len()),
    );

    let mut sessions = Vec::with_capacity(cams.len());
    for (i, c) in cams.iter().enumerate() {
        let s = block_on(c.open_ptp())?;
        log(
            t0,
            &format!(
                "  [{i}] session open  bus {} addr {}  serial {}",
                c.bus_number(),
                c.device_address(),
                c.serial().unwrap_or("?")
            ),
        );
        sessions.push(s);
    }

    let n = sessions.len();
    let barrier_offsets_done = Arc::new(Barrier::new(n));
    let barrier_target_set = Arc::new(Barrier::new(n));
    let target_host_ms: Arc<Mutex<f64>> = Arc::new(Mutex::new(0.0));
    let timings: Arc<Mutex<Vec<CamTiming>>> = Arc::new(Mutex::new(Vec::with_capacity(n)));

    log(t0, &format!("spawning {n} camera thread(s)…"));

    let handles: Vec<_> = sessions
        .into_iter()
        .enumerate()
        .map(|(i, mut s)| {
            let label = format!(
                "[{i} {}]",
                cams[i].serial().unwrap_or("?").get(..8).unwrap_or("?")
            );
            let b1 = barrier_offsets_done.clone();
            let b2 = barrier_target_set.clone();
            let target = target_host_ms.clone();
            let timings_arc = timings.clone();

            thread::spawn(move || -> Result<()> {
                let mut t = CamTiming {
                    label: label.clone(),
                    ..Default::default()
                };

                // ---------- Phase 1: measure tick offset (NTP-style) ----------
                let (offset_ms, rtt_ms) = block_on(measure_offset(&mut s, t0, &label))?;
                t.offset_ms = offset_ms;
                t.offset_rtt_ms = rtt_ms;
                log(
                    t0,
                    &format!(
                        "{label} offset = {:.2}ms  (best RTT {:.2}ms over {N_OFFSET_SAMPLES} samples)",
                        offset_ms, rtt_ms
                    ),
                );

                // ---------- Phase 2: barrier, leader computes target ----------
                let was_leader = b1.wait().is_leader();
                if was_leader {
                    let target_set = host_ms(t0) + TARGET_LEAD_MS;
                    *target.lock().unwrap() = target_set;
                    log(t0, &format!("LEADER: target host_ms = {:.1}", target_set));
                }
                b2.wait();

                let target_host = *target.lock().unwrap();
                let target_tick = (target_host + offset_ms).round() as u64;
                t.target_tick = target_tick;

                // ---------- Phase 3: dispatch combined script ----------
                let lua = combined_lua(target_tick);
                t.script_send_ms = host_ms(t0);
                log(
                    t0,
                    &format!(
                        "{label} combined SEND  (target_tick={target_tick}, slack={:.1}ms)",
                        target_host - host_ms(t0)
                    ),
                );
                let msgs = block_on(s.execute_script_wait(&lua, 25_000))?;
                t.script_recv_ms = host_ms(t0);
                collect_errors(&label, "combined", &msgs, &mut t.errors);

                let ret = first_return_string(&msgs);
                if let Some(parts) = ret.as_deref().and_then(parse_combined_return) {
                    t.cam_tick_start = Some(parts.0);
                    t.cam_tick_warmup_done = Some(parts.1);
                    t.cam_tick_exit = Some(parts.2);
                    t.cam_tick_done = Some(parts.3);
                    t.exp_at_start = Some(parts.4);
                    t.exp_after_mode = Some(parts.5);
                    t.exp_after_half = Some(parts.6);
                    t.exp_before_fire = Some(parts.7);
                    t.exp_after = Some(parts.8);
                    t.actual_exit_host_ms = Some(parts.2 as f64 - offset_ms);
                }
                log(
                    t0,
                    &format!(
                        "{label} combined RECV ({:.1}ms rtt) → {:?}  actual_exit_host={:?}",
                        t.script_recv_ms - t.script_send_ms,
                        ret.as_deref().unwrap_or("?"),
                        t.actual_exit_host_ms.map(|m| format!("{m:.1}ms")),
                    ),
                );

                timings_arc.lock().unwrap().push(t);
                Ok(())
            })
        })
        .collect();

    let mut failed = 0;
    for h in handles {
        if let Err(e) = h.join().expect("thread panicked") {
            failed += 1;
            eprintln!("camera thread error: {e}");
        }
    }
    log(
        t0,
        &format!(
            "all threads joined ({}/{} ok, total elapsed {:.2}s)",
            n - failed,
            n,
            t0.elapsed().as_secs_f64()
        ),
    );

    print_summary(&timings.lock().unwrap(), *target_host_ms.lock().unwrap());
    Ok(())
}

async fn measure_offset(
    s: &mut chdkptp::PtpSession,
    t0: Instant,
    label: &str,
) -> Result<(f64, f64)> {
    let mut best: Option<(f64, f64)> = None;
    for i in 0..N_OFFSET_SAMPLES {
        let t1 = host_ms(t0);
        let msgs = s.execute_script_wait(TICK_PROBE_LUA, 2000).await?;
        let t2 = host_ms(t0);
        let rtt = t2 - t1;
        let cam_tick = msgs.iter().find_map(|m| match m {
            ScriptMsg::Return {
                value: ScriptValue::Integer(v),
                ..
            } => Some(*v as i64 as f64),
            _ => None,
        });
        let Some(cam_tick) = cam_tick else { continue };
        let host_mid = (t1 + t2) / 2.0;
        let offset = cam_tick - host_mid;
        log(
            t0,
            &format!(
                "{label}   probe[{i}]: rtt={:.2}ms  cam_tick={cam_tick:.0}  offset={:.2}ms",
                rtt, offset
            ),
        );
        match best {
            None => best = Some((offset, rtt)),
            Some((_, br)) if rtt < br => best = Some((offset, rtt)),
            _ => {}
        }
    }
    Ok(best.expect("at least one offset sample"))
}

fn host_ms(t0: Instant) -> f64 {
    t0.elapsed().as_secs_f64() * 1000.0
}

fn log(t0: Instant, msg: &str) {
    println!("[+{:>8.1}ms] {msg}", host_ms(t0));
}

fn collect_errors(label: &str, phase: &str, msgs: &[ScriptMsg], dest: &mut Vec<String>) {
    for m in msgs {
        if let ScriptMsg::Error { text, category, .. } = m {
            let s = format!("{phase} ERR [{category:?}]: {text}");
            eprintln!("  {label} {s}");
            dest.push(s);
        }
    }
}

fn first_return_string(msgs: &[ScriptMsg]) -> Option<String> {
    msgs.iter().find_map(|m| match m {
        ScriptMsg::Return {
            value: ScriptValue::String(s),
            ..
        } => Some(s.clone()),
        _ => None,
    })
}

/// Parse the 9-tuple return from `combined_lua`:
/// `"<t_start>,<warmup_done>,<t_exit>,<t_done>,
///   <exp_at_start>,<exp_after_mode>,<exp_after_half>,<exp_before_fire>,<exp_after>"`.
fn parse_combined_return(s: &str) -> Option<(u64, u64, u64, u64, u64, u64, u64, u64, u64)> {
    let mut parts = s.split(',');
    let a = parts.next()?.parse().ok()?;
    let b = parts.next()?.parse().ok()?;
    let c = parts.next()?.parse().ok()?;
    let d = parts.next()?.parse().ok()?;
    let e = parts.next()?.parse().ok()?;
    let f = parts.next()?.parse().ok()?;
    let g = parts.next()?.parse().ok()?;
    let h = parts.next()?.parse().ok()?;
    let i = parts.next()?.parse().ok()?;
    Some((a, b, c, d, e, f, g, h, i))
}

fn print_summary(ts: &[CamTiming], target_host: f64) {
    println!();
    println!("=================== clock-sync timing summary ===================");
    println!("Target host_ms: {:.1}", target_host);
    println!();
    println!(
        "{:<14} {:>10} {:>10} {:>10} {:>10} {:>10} {:>14}",
        "camera", "offset RTT", "warmup", "busy-wait", "actual exit", "overshoot", "shutter?"
    );
    println!("{}", "-".repeat(94));
    for t in ts {
        let warmup_dur = match (t.cam_tick_start, t.cam_tick_warmup_done) {
            (Some(a), Some(b)) => format!("{}ms", b.saturating_sub(a)),
            _ => "?".into(),
        };
        let busy_wait_dur = match (t.cam_tick_warmup_done, t.cam_tick_exit) {
            (Some(a), Some(b)) => format!("{}ms", b.saturating_sub(a)),
            _ => "?".into(),
        };
        let overshoot = match t.actual_exit_host_ms {
            Some(actual) => format!("{:+.1}ms", actual - target_host),
            None => "?".into(),
        };
        let actual = t
            .actual_exit_host_ms
            .map(|m| format!("{m:.1}ms"))
            .unwrap_or_else(|| "?".into());
        let shutter = match (t.exp_at_start, t.exp_after) {
            (Some(b), Some(a)) if a > b => format!("FIRED (+{})", a - b),
            (Some(b), Some(a)) if a == b => "MISSED".into(),
            (Some(b), Some(a)) => format!("?? ({b}→{a})"),
            _ => "?".into(),
        };
        println!(
            "{:<14} {:>8.2}ms {:>10} {:>10} {:>10} {:>10} {:>14}",
            t.label, t.offset_rtt_ms, warmup_dur, busy_wait_dur, actual, overshoot, shutter
        );
    }
    println!();
    println!("=== exp_count checkpoints (when did extra shutter actuations happen?) ===");
    println!(
        "{:<14} {:>10} {:>12} {:>13} {:>14} {:>10}",
        "camera", "at start", "after mode", "after half", "before fire", "after"
    );
    println!("{}", "-".repeat(78));
    for t in ts {
        let fmt = |o: Option<u64>| o.map(|v| v.to_string()).unwrap_or_else(|| "?".into());
        println!(
            "{:<14} {:>10} {:>12} {:>13} {:>14} {:>10}",
            t.label,
            fmt(t.exp_at_start),
            fmt(t.exp_after_mode),
            fmt(t.exp_after_half),
            fmt(t.exp_before_fire),
            fmt(t.exp_after),
        );
    }
    println!();
    println!("Legend:");
    println!("  offset RTT    round-trip of best offset probe (smaller = more accurate)");
    println!("  warmup        camera-side time for mode switch + flash arm + AF lock");
    println!("  busy-wait     camera-side time in the spin loop (warmup→target)");
    println!("                if 0, warmup overran target — bump TARGET_LEAD_MS");
    println!("  actual exit   busy-wait exit on camera, converted to host wall-clock");
    println!("  overshoot     actual_exit - target_host (positive = late)");
    println!();
    println!("exp_count checkpoint columns let you pinpoint when a stray shot fired:");
    println!("  at start → after mode     : mode switch caused a shot");
    println!("  after mode → after half   : the half-press caused a shot (drive mode? burst?)");
    println!("  after half → before fire  : something during busy-wait fired (shouldn't happen)");
    println!("  before fire → after       : the intended shot — should be exactly +1");
    println!();

    if ts.len() >= 2 {
        let actuals: Vec<f64> = ts.iter().filter_map(|t| t.actual_exit_host_ms).collect();
        if actuals.len() == ts.len() {
            let min = actuals.iter().cloned().fold(f64::INFINITY, f64::min);
            let max = actuals.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            println!(
                "*** Inter-camera shutter-arming skew: {:.2}ms ***",
                max - min
            );
        }
    }
}
