//! Clock-sync variant of shoot_all.
//!
//! Per camera:
//!   1. Warm-up (mode → record, flash on, half-press held).
//!   2. Measure tick offset between host wall-clock and camera tick
//!      (NTP-style, multiple samples, pick best RTT).
//!   3. All threads converge on a shared target wall-clock time T.
//!   4. Each thread sends a script that BUSY-WAITS until its local tick
//!      equals (T + offset), then fires the shutter.
//!
//! The synchronization point moves from "host sends trigger" (subject to
//! USB jitter) to "each camera watches its own clock" (sub-ms precision
//! bounded by tick-offset measurement error and busy-wait granularity).

use chdkptp::chdk::{ScriptMsg, ScriptValue};
use chdkptp::{list_cameras, Result};
use pollster::block_on;
use std::sync::{Arc, Barrier, Mutex};
use std::thread;
use std::time::Instant;

const N_OFFSET_SAMPLES: usize = 7;

/// How far in the future to schedule the synchronized shot, in host ms.
/// Must exceed the time it takes to dispatch the fire-at script to all
/// cameras — generous default for ≤10 cameras.
const TARGET_LEAD_MS: f64 = 800.0;

/// Warm-up: enter record (with extended settle), force flash, half-press HOLD.
/// Returns `"<status>,<exp_count>"` so we have a pre-shot baseline.
const WARMUP_LUA: &str = "\
    if not get_mode() then \
      switch_mode_usb(1) \
      sleep(3500) \
    end \
    local ok, p = pcall(require, 'propcase') \
    if ok then set_prop(p.FLASH_MODE, 1) end \
    press('shoot_half') \
    local t = get_tick_count() \
    while not get_shooting() and (get_tick_count() - t) < 5000 do \
      sleep(50) \
    end \
    sleep(200) \
    local status = get_shooting() and 'armed' or 'no_focus_lock' \
    return status .. ',' .. get_exp_count()";

/// Offset probe: just return the camera's current tick count.
const TICK_PROBE_LUA: &str = "return get_tick_count()";

/// Build the per-camera fire-at-tick script. The target tick is baked in.
///
/// Sequence: busy-wait → re-press shoot_half → SETTLE (critical — some
/// cameras' input layer drops the original multi-second hold and needs the
/// re-press to register before shoot_full arrives) → press shoot_full →
/// hold → release → wait for save → read exp_count.
///
/// Returns `"<t_exit>,<t_done>,<exp_before>,<exp_after>"`.
fn fire_at_lua(target_tick: u64) -> String {
    format!(
        "local t = {target_tick} \
         while get_tick_count() < t do end \
         local t_exit = get_tick_count() \
         local exp_before = get_exp_count() \
         press('shoot_half') \
         sleep(80) \
         press('shoot_full') \
         sleep(150) \
         release('shoot_full') \
         release('shoot_half') \
         local t_done = get_tick_count() \
         sleep(1800) \
         local exp_after = get_exp_count() \
         return t_exit .. ',' .. t_done .. ',' .. exp_before .. ',' .. exp_after"
    )
}

#[derive(Default, Debug)]
#[allow(dead_code)]
struct CamTiming {
    label: String,
    warmup_dur_ms: f64,
    offset_ms: f64,
    offset_rtt_ms: f64,
    target_tick: u64,
    fire_send_ms: f64,
    fire_recv_ms: f64,
    cam_tick_exit: Option<u64>,
    cam_tick_done: Option<u64>,
    actual_exit_host_ms: Option<f64>, // tick_exit converted back to host time
    exp_before: Option<u64>,
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

                // ---------- Phase 1: warm-up + hold half-press ----------
                let w0 = host_ms(t0);
                log(t0, &format!("{label} warm-up START"));
                let warmup_msgs = block_on(s.execute_script_wait(WARMUP_LUA, 15_000))?;
                t.warmup_dur_ms = host_ms(t0) - w0;
                collect_errors(&label, "warm-up", &warmup_msgs, &mut t.errors);
                let armed = first_return_string(&warmup_msgs).unwrap_or_default();
                log(
                    t0,
                    &format!(
                        "{label} warm-up END ({:.1}ms) → {armed:?}",
                        t.warmup_dur_ms
                    ),
                );

                // ---------- Phase 2: measure tick offset (NTP-style) ----------
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

                // ---------- Phase 3: barrier — all offsets known ----------
                let was_leader = b1.wait().is_leader();
                if was_leader {
                    let target_set = host_ms(t0) + TARGET_LEAD_MS;
                    *target.lock().unwrap() = target_set;
                    log(t0, &format!("LEADER: target host_ms = {:.1}", target_set));
                }
                b2.wait(); // all read the same target

                let target_host = *target.lock().unwrap();
                let target_tick = (target_host + offset_ms).round() as u64;
                t.target_tick = target_tick;

                // ---------- Phase 4: fire at the local target tick ----------
                let lua = fire_at_lua(target_tick);
                t.fire_send_ms = host_ms(t0);
                log(
                    t0,
                    &format!(
                        "{label} fire SEND  (target_tick={target_tick}, slack={:.1}ms)",
                        target_host - host_ms(t0)
                    ),
                );
                let fire_msgs = block_on(s.execute_script_wait(&lua, 15_000))?;
                t.fire_recv_ms = host_ms(t0);
                collect_errors(&label, "fire", &fire_msgs, &mut t.errors);

                let ret = first_return_string(&fire_msgs);
                if let Some(parts) = ret.as_deref().and_then(parse_fire_return) {
                    t.cam_tick_exit = Some(parts.0);
                    t.cam_tick_done = Some(parts.1);
                    t.exp_before = Some(parts.2);
                    t.exp_after = Some(parts.3);
                    t.actual_exit_host_ms = Some(parts.0 as f64 - offset_ms);
                }
                log(
                    t0,
                    &format!(
                        "{label} fire RECV ({:.1}ms rtt) → {:?}  actual_exit_host={:?}",
                        t.fire_recv_ms - t.fire_send_ms,
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

/// NTP-style offset estimate. Returns `(offset_ms, best_rtt_ms)`.
///
/// `offset_ms` is defined as `camera_tick - host_ms_at_midpoint`, so the
/// conversion back is: `host_ms = camera_tick - offset_ms`.
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

/// Parse `"<t_exit>,<t_done>,<exp_before>,<exp_after>"`.
fn parse_fire_return(s: &str) -> Option<(u64, u64, u64, u64)> {
    let mut parts = s.split(',');
    let a = parts.next()?.parse().ok()?;
    let b = parts.next()?.parse().ok()?;
    let c = parts.next()?.parse().ok()?;
    let d = parts.next()?.parse().ok()?;
    Some((a, b, c, d))
}

fn print_summary(ts: &[CamTiming], target_host: f64) {
    println!();
    println!("=================== clock-sync timing summary ===================");
    println!("Target host_ms: {:.1}", target_host);
    println!();
    println!(
        "{:<14} {:>9} {:>10} {:>10} {:>10} {:>10}",
        "camera", "warm-up", "offset RTT", "actual exit", "overshoot", "shutter?"
    );
    println!("{}", "-".repeat(74));
    for t in ts {
        let overshoot = match t.actual_exit_host_ms {
            Some(actual) => format!("{:+.1}ms", actual - target_host),
            None => "?".into(),
        };
        let actual = t
            .actual_exit_host_ms
            .map(|m| format!("{m:.1}ms"))
            .unwrap_or_else(|| "?".into());
        let shutter = match (t.exp_before, t.exp_after) {
            (Some(b), Some(a)) if a > b => format!("FIRED (+{})", a - b),
            (Some(b), Some(a)) if a == b => "MISSED".to_string(),
            (Some(b), Some(a)) => format!("?? ({b}→{a})"),
            _ => "?".to_string(),
        };
        println!(
            "{:<14} {:>8.1}ms {:>8.2}ms {:>10} {:>10} {:>10}",
            t.label, t.warmup_dur_ms, t.offset_rtt_ms, actual, overshoot, shutter
        );
    }
    println!();
    println!("Legend:");
    println!("  offset        camera_tick - host_ms (NTP midpoint estimate)");
    println!("  offset RTT    round-trip time of the offset probe (lower = more accurate)");
    println!("  actual exit   busy-wait exit on the camera, converted back to host wall-clock");
    println!("  overshoot     actual_exit - target_host (positive = late, negative = early)");
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
            println!("(this is the precision of the synchronization — what the barrier approach");
            println!(" cannot do, and what physically limits how close the actual shutters fire)");
        }
    }
}
