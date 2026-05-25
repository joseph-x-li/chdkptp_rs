//! Synchronized shoot across every connected Canon camera, with verbose
//! per-event timestamping.
//!
//! All host-side timestamps are `[+NNNN.Nms]` relative to program start.
//! Camera-side ticks (`get_tick_count()`) are returned by the FIRE script so we
//! can measure camera-internal latency precisely. NOTE: each camera's tick
//! counter is independent (counts from its own boot), so absolute ticks
//! cannot be compared across cameras — only DURATIONS within a single camera.

use chdkptp::chdk::{ScriptMsg, ScriptValue};
use chdkptp::{list_cameras, Result};
use pollster::block_on;
use std::sync::{Arc, Barrier, Mutex};
use std::thread;
use std::time::Instant;

/// Warm-up: enter record mode, force flash ON, half-press and HOLD it.
/// Returns "armed,<camera_tick>" or "no_focus_lock,<camera_tick>".
const WARMUP_LUA: &str = "\
    if not get_mode() then \
      switch_mode_usb(1) \
      sleep(2000) \
    end \
    local ok, p = pcall(require, 'propcase') \
    if ok then set_prop(p.FLASH_MODE, 1) end \
    press('shoot_half') \
    local t = get_tick_count() \
    while not get_shooting() and (get_tick_count() - t) < 5000 do \
      sleep(50) \
    end \
    return (get_shooting() and 'armed' or 'no_focus_lock') \
           .. ',' .. get_tick_count()";

/// Fire: shutter press while shoot_half is held. Returns
/// "<tick_at_fire_start>,<tick_at_fire_end>" — host can compute camera-side
/// execution time as the difference.
const FIRE_LUA: &str = "\
    local t0 = get_tick_count() \
    press('shoot_full') \
    sleep(50) \
    release('shoot_full') \
    release('shoot_half') \
    local t1 = get_tick_count() \
    return t0 .. ',' .. t1";

/// Per-camera timing record collected by each thread.
#[derive(Default, Debug)]
#[allow(dead_code)]
struct CamTiming {
    label: String,
    open_ms: f64,
    warmup_start_ms: f64,
    warmup_end_ms: f64,
    barrier_enter_ms: f64,
    barrier_exit_ms: f64,
    fire_send_ms: f64,
    fire_recv_ms: f64,
    armed_status: String,
    camera_tick_after_warmup: Option<u64>,
    camera_tick_fire_start: Option<u64>,
    camera_tick_fire_end: Option<u64>,
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
    for (i, c) in cams.iter().enumerate() {
        log(
            t0,
            &format!(
                "  [{i}] bus {} addr {} serial {}",
                c.bus_number(),
                c.device_address(),
                c.serial().unwrap_or("?")
            ),
        );
    }

    // Open all PTP sessions on the main thread (fast — ms each).
    log(t0, "opening PTP sessions…");
    let mut sessions = Vec::with_capacity(cams.len());
    for (i, c) in cams.iter().enumerate() {
        let t_before = t0.elapsed().as_secs_f64() * 1000.0;
        let s = block_on(c.open_ptp())?;
        let t_after = t0.elapsed().as_secs_f64() * 1000.0;
        log(
            t0,
            &format!("  [{i}] session open ({:.1}ms)", t_after - t_before),
        );
        sessions.push(s);
    }

    let n = sessions.len();
    let barrier = Arc::new(Barrier::new(n));
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
            let b = barrier.clone();
            let timings_arc = timings.clone();
            thread::spawn(move || -> Result<()> {
                let mut t = CamTiming {
                    label: label.clone(),
                    open_ms: t0.elapsed().as_secs_f64() * 1000.0,
                    ..Default::default()
                };

                // --- Warm-up ---
                t.warmup_start_ms = t0.elapsed().as_secs_f64() * 1000.0;
                log(t0, &format!("{label} warm-up START"));
                let warmup_msgs = block_on(s.execute_script_wait(WARMUP_LUA, 15_000))?;
                t.warmup_end_ms = t0.elapsed().as_secs_f64() * 1000.0;
                collect_errors(&label, "warm-up", &warmup_msgs, &mut t.errors);

                let warmup_return = first_return_string(&warmup_msgs);
                if let Some((status, tick)) = warmup_return.as_deref().and_then(parse_armed) {
                    t.armed_status = status.to_string();
                    t.camera_tick_after_warmup = Some(tick);
                }
                log(
                    t0,
                    &format!(
                        "{label} warm-up END  (+{:.1}ms duration) → {:?}",
                        t.warmup_end_ms - t.warmup_start_ms,
                        warmup_return.as_deref().unwrap_or("?"),
                    ),
                );

                // --- Barrier rendezvous ---
                t.barrier_enter_ms = t0.elapsed().as_secs_f64() * 1000.0;
                log(t0, &format!("{label} barrier.wait() ENTER"));
                let wait_result = b.wait();
                t.barrier_exit_ms = t0.elapsed().as_secs_f64() * 1000.0;
                log(
                    t0,
                    &format!(
                        "{label} barrier.wait() EXIT  (waited {:.2}ms, leader={})",
                        t.barrier_exit_ms - t.barrier_enter_ms,
                        wait_result.is_leader()
                    ),
                );

                // --- Synchronized fire ---
                t.fire_send_ms = t0.elapsed().as_secs_f64() * 1000.0;
                log(t0, &format!("{label} fire SEND"));
                let fire_msgs = block_on(s.execute_script_wait(FIRE_LUA, 15_000))?;
                t.fire_recv_ms = t0.elapsed().as_secs_f64() * 1000.0;
                collect_errors(&label, "fire", &fire_msgs, &mut t.errors);

                let fire_return = first_return_string(&fire_msgs);
                if let Some((a, b)) = fire_return.as_deref().and_then(parse_two_ticks) {
                    t.camera_tick_fire_start = Some(a);
                    t.camera_tick_fire_end = Some(b);
                }
                log(
                    t0,
                    &format!(
                        "{label} fire RECV  (host-rtt {:.1}ms) → {:?}",
                        t.fire_recv_ms - t.fire_send_ms,
                        fire_return.as_deref().unwrap_or("?"),
                    ),
                );

                timings_arc.lock().unwrap().push(t);
                Ok(())
            })
        })
        .collect();

    let mut failures = 0;
    for h in handles {
        if let Err(e) = h.join().expect("thread panicked") {
            failures += 1;
            eprintln!("camera thread error: {e}");
        }
    }

    log(
        t0,
        &format!(
            "all threads joined ({}/{} ok, total elapsed {:.2}s)",
            n - failures,
            n,
            t0.elapsed().as_secs_f64()
        ),
    );

    print_summary(&timings.lock().unwrap());

    Ok(())
}

fn log(t0: Instant, msg: &str) {
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    // One println! call → atomic w.r.t. other threads writing to stdout.
    println!("[+{ms:>8.1}ms] {msg}");
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

fn parse_armed(s: &str) -> Option<(&str, u64)> {
    let (status, tick) = s.split_once(',')?;
    Some((status, tick.parse().ok()?))
}

fn parse_two_ticks(s: &str) -> Option<(u64, u64)> {
    let (a, b) = s.split_once(',')?;
    Some((a.parse().ok()?, b.parse().ok()?))
}

fn print_summary(ts: &[CamTiming]) {
    println!();
    println!("=================== timing summary ===================");

    // Reference: latest barrier-exit across all cameras (they should be within μs)
    let max_barrier_exit = ts.iter().map(|t| t.barrier_exit_ms).fold(0.0_f64, f64::max);

    println!(
        "{:<14} {:>10} {:>10} {:>10} {:>10} {:>13} {:>12}",
        "camera", "warm-up", "barrier↑", "rel.barrier", "fire-rtt", "cam-side fire", "errors"
    );
    println!("{}", "-".repeat(86));
    for t in ts {
        let warmup_dur = t.warmup_end_ms - t.warmup_start_ms;
        let barrier_wait = t.barrier_exit_ms - t.barrier_enter_ms;
        // How long after the last camera exited the barrier did THIS camera's
        // fire script complete? Tight is good.
        let fire_relative_to_barrier = t.fire_recv_ms - max_barrier_exit;
        let fire_rtt = t.fire_recv_ms - t.fire_send_ms;
        let cam_side = match (t.camera_tick_fire_start, t.camera_tick_fire_end) {
            (Some(a), Some(b)) => format!("{}ms", b.saturating_sub(a)),
            _ => "?".to_string(),
        };
        println!(
            "{:<14} {:>9.1}ms {:>9.1}ms {:>9.1}ms {:>8.1}ms {:>13} {:>12}",
            t.label,
            warmup_dur,
            barrier_wait,
            fire_relative_to_barrier,
            fire_rtt,
            cam_side,
            t.errors.len()
        );
    }

    println!();
    println!("Legend:");
    println!("  warm-up      camera-internal time to mode-switch + flash-on + AF-lock");
    println!("  barrier↑     this thread's wait at the barrier (last thread = ~0)");
    println!("  rel.barrier  host-time from last-barrier-exit until fire response received");
    println!("  fire-rtt     host-side round-trip for the FIRE script (send → response)");
    println!("  cam-side     camera-internal duration of the FIRE script (tick delta)");
    println!();

    // Inter-camera variance metrics
    if ts.len() >= 2 {
        let rels: Vec<f64> = ts
            .iter()
            .map(|t| t.fire_recv_ms - max_barrier_exit)
            .collect();
        let min = rels.iter().cloned().fold(f64::INFINITY, f64::min);
        let max = rels.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        println!(
            "Inter-camera skew (host-side, fire-completion): {:.1}ms",
            max - min
        );

        let cam_sides: Vec<u64> = ts
            .iter()
            .filter_map(|t| {
                Some(
                    t.camera_tick_fire_end?
                        .saturating_sub(t.camera_tick_fire_start?),
                )
            })
            .collect();
        if cam_sides.len() == ts.len() {
            let cmin = *cam_sides.iter().min().unwrap();
            let cmax = *cam_sides.iter().max().unwrap();
            println!(
                "Inter-camera variance (camera-side FIRE duration): {}ms (min {}ms, max {}ms)",
                cmax - cmin,
                cmin,
                cmax
            );
            println!("  → if this is tight, all cameras did the same amount of internal work.");
            println!(
                "  → cross-camera SHUTTER timing skew is bounded above by rel.barrier max−min."
            );
        }
    }
}
