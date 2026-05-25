//! CHDK-specific methods, added to `PtpSession` via additional inherent impl blocks.

use super::opcode::{self, Sub, PTP_OC_CHDK};
use super::script::{ErrorCategory, ScriptId, ScriptMsg, ScriptStatus, ScriptValue};
use crate::ptp::{DataPhase, PtpSession};
use crate::{Error, Result};

/// CHDK PTP protocol version reported by the camera.
#[derive(Debug, Clone, Copy)]
pub struct ChdkVersion {
    pub major: u32,
    pub minor: u32,
}

impl std::fmt::Display for ChdkVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

impl PtpSession {
    // ---------- Version ----------

    /// Query the CHDK PTP protocol version (sub-command 0, no data phase).
    pub async fn chdk_version(&mut self) -> Result<ChdkVersion> {
        let resp = self
            .command(PTP_OC_CHDK, &[Sub::Version.as_u32()], DataPhase::None)
            .await?;
        resp.ok()?;
        if resp.params.len() < 2 {
            return Err(Error::Codec(format!(
                "chdk_version: expected ≥2 response params, got {}",
                resp.params.len()
            )));
        }
        Ok(ChdkVersion {
            major: resp.params[0],
            minor: resp.params[1],
        })
    }

    // ---------- Script execution ----------

    /// Send Lua source to the camera's on-board interpreter and return the
    /// assigned `ScriptId`. Returns immediately — the script runs asynchronously
    /// on the camera; use `script_status()` and `read_script_msg()` to drive it.
    pub async fn execute_script_lua(&mut self, source: &str) -> Result<ScriptId> {
        let param2 = opcode::SCRIPT_LANG_LUA as u32
            | opcode::SCRIPT_FLAG_FLUSH_CAM_MSGS
            | opcode::SCRIPT_FLAG_FLUSH_HOST_MSGS;
        // CHDK loads scripts via luaL_loadstring (NUL-terminated). Without the
        // trailing NUL the parser walks into stale buffer contents from prior runs.
        let mut payload = Vec::with_capacity(source.len() + 1);
        payload.extend_from_slice(source.as_bytes());
        payload.push(0);
        let resp = self
            .command(
                PTP_OC_CHDK,
                &[Sub::ExecuteScript.as_u32(), param2],
                DataPhase::Out(&payload),
            )
            .await?;
        resp.ok()?;
        if resp.params.is_empty() {
            return Err(Error::Codec(
                "execute_script_lua: response missing script_id".into(),
            ));
        }
        Ok(resp.params[0])
    }

    /// Poll the running script's status (RUN bit, MSG-pending bit).
    pub async fn script_status(&mut self) -> Result<ScriptStatus> {
        let resp = self
            .command(PTP_OC_CHDK, &[Sub::ScriptStatus.as_u32()], DataPhase::None)
            .await?;
        resp.ok()?;
        let raw = resp.params.first().copied().unwrap_or(0);
        Ok(ScriptStatus::from_raw(raw))
    }

    /// Drain one message from the camera's outbound queue.
    /// Returns `ScriptMsg::None` if the queue is empty.
    pub async fn read_script_msg(&mut self) -> Result<ScriptMsg> {
        let resp = self
            .command(PTP_OC_CHDK, &[Sub::ReadScriptMsg.as_u32()], DataPhase::In)
            .await?;
        resp.ok()?;

        let msg_type = resp.params.first().copied().unwrap_or(opcode::MSGTYPE_NONE);
        let subtype = resp.params.get(1).copied().unwrap_or(0);
        let script_id = resp.params.get(2).copied().unwrap_or(0);
        // params[3] would be length; resp.data already carries the payload.

        Ok(match msg_type {
            opcode::MSGTYPE_NONE => ScriptMsg::None,
            opcode::MSGTYPE_ERR => ScriptMsg::Error {
                script_id,
                category: ErrorCategory::from_raw(subtype),
                text: String::from_utf8_lossy(&resp.data).into_owned(),
            },
            opcode::MSGTYPE_RET => ScriptMsg::Return {
                script_id,
                value: ScriptValue::decode(subtype, &resp.data),
            },
            opcode::MSGTYPE_USER => ScriptMsg::User {
                script_id,
                value: ScriptValue::decode(subtype, &resp.data),
            },
            other => {
                return Err(Error::Codec(format!(
                    "read_script_msg: unknown msg_type {other}"
                )))
            }
        })
    }

    /// Convenience: execute Lua source, poll until the script finishes
    /// (or `timeout_ms` elapses), drain all messages, return them in order.
    ///
    /// Errors in the returned vec do NOT abort — callers can inspect the full
    /// transcript. Returns `Err(...)` only on transport / timeout failures.
    pub async fn execute_script_wait(
        &mut self,
        source: &str,
        timeout_ms: u64,
    ) -> Result<Vec<ScriptMsg>> {
        let _script_id = self.execute_script_lua(source).await?;
        let mut msgs = Vec::new();
        let start = std::time::Instant::now();
        // 20ms keeps probe RTTs tight (matters for clock-sync offset estimation
        // and any other timing-sensitive use). For a typical 1–3s shoot()
        // the extra polling cost is negligible (~50–150 idle queries vs 7–20).
        let poll_interval = std::time::Duration::from_millis(20);

        loop {
            // Drain any pending messages first.
            loop {
                let m = self.read_script_msg().await?;
                if matches!(m, ScriptMsg::None) {
                    break;
                }
                msgs.push(m);
            }

            let status = self.script_status().await?;
            if !status.running() {
                // Final drain after the script ends (RET / late ERR).
                loop {
                    let m = self.read_script_msg().await?;
                    if matches!(m, ScriptMsg::None) {
                        break;
                    }
                    msgs.push(m);
                }
                return Ok(msgs);
            }

            if start.elapsed() > std::time::Duration::from_millis(timeout_ms) {
                return Err(Error::Usb(format!(
                    "script timed out after {timeout_ms} ms"
                )));
            }

            std::thread::sleep(poll_interval);
        }
    }

    /// Take a picture. Sends `shoot()` to the camera, waits for completion,
    /// returns the full script transcript (errors included).
    ///
    /// If the camera is in playback mode, switches it to record first
    /// (this adds ~2 s of warm-up time).
    pub async fn shoot(&mut self) -> Result<Vec<ScriptMsg>> {
        let src = "\
            if not get_mode() then \
              switch_mode_usb(1) \
              sleep(2000) \
            end \
            shoot()";
        self.execute_script_wait(src, 20_000).await
    }
}
