//! PTP session: opens the USB device, claims the still-image interface,
//! and pumps Command / Data / Response containers over the bulk endpoints.

use super::container::{self, Decoded};
use super::device_info::DeviceInfo;
use super::opcode;
use crate::{Error, Result};
use nusb::transfer::{Direction, EndpointType, RequestBuffer};

const READ_CHUNK: usize = 16 * 1024;

#[derive(Debug, Clone, Copy)]
pub enum DataPhase<'a> {
    None,
    /// Camera will send a Data container after the Command.
    In,
    /// Host sends a Data container after the Command.
    Out(&'a [u8]),
}

#[derive(Debug)]
pub struct PtpResponse {
    pub code: u16,
    pub params: Vec<u32>,
    /// Bytes from the IN data phase, if any. Empty otherwise.
    pub data: Vec<u8>,
}

impl PtpResponse {
    pub fn ok(&self) -> Result<()> {
        if self.code == opcode::RC_OK {
            Ok(())
        } else {
            Err(Error::PtpResponse { code: self.code })
        }
    }
}

pub struct PtpSession {
    interface: nusb::Interface,
    ep_in: u8,
    ep_out: u8,
    transaction_id: u32,
    session_open: bool,
}

impl PtpSession {
    /// Open a PTP session over the given USB device.
    ///
    /// Finds the still-image interface (class 6 / subclass 1 / proto 1), claims it,
    /// discovers the bulk IN/OUT endpoints, and sends `OpenSession`.
    pub async fn open(info: &nusb::DeviceInfo) -> Result<Self> {
        let device = info.open()?;
        let config = device
            .active_configuration()
            .map_err(|e| Error::Usb(format!("active_configuration: {e}")))?;
        let (ifnum, ep_in, ep_out) = find_ptp_endpoints(&config)?;
        let interface = device
            .claim_interface(ifnum)
            .map_err(|e| Error::Usb(format!("claim_interface({ifnum}): {e}")))?;

        let mut s = Self {
            interface,
            ep_in,
            ep_out,
            transaction_id: 0,
            session_open: false,
        };
        let resp = s
            .command(opcode::OP_OPEN_SESSION, &[1], DataPhase::None)
            .await?;
        // Some cameras return Session_Already_Open if a previous session wasn't cleanly closed;
        // treat that as success since we now own this session.
        if resp.code != opcode::RC_OK && resp.code != opcode::RC_SESSION_ALREADY_OPEN {
            return Err(Error::PtpResponse { code: resp.code });
        }
        s.session_open = true;
        Ok(s)
    }

    pub async fn get_device_info(&mut self) -> Result<DeviceInfo> {
        let resp = self
            .command(opcode::OP_GET_DEVICE_INFO, &[], DataPhase::In)
            .await?;
        resp.ok()?;
        DeviceInfo::decode(&resp.data)
    }

    /// Send a PTP transaction: Command [+ Data] + Response.
    pub async fn command(
        &mut self,
        code: u16,
        params: &[u32],
        data_phase: DataPhase<'_>,
    ) -> Result<PtpResponse> {
        let txn = self.transaction_id;
        self.transaction_id = self.transaction_id.wrapping_add(1);

        // Command container
        let cmd = container::encode(opcode::CONTAINER_COMMAND, code, txn, params, &[]);
        self.bulk_out(cmd).await?;

        // OUT data phase, if any
        if let DataPhase::Out(data) = data_phase {
            let dc = container::encode(opcode::CONTAINER_DATA, code, txn, &[], data);
            self.bulk_out(dc).await?;
        }

        // IN data phase, if expected. PTP spec allows the responder to skip
        // the data phase entirely (e.g. if there's nothing to return or an
        // error occurred) — in that case the next container is the Response
        // and we treat the call as "no data, see response code".
        let in_data = if matches!(data_phase, DataPhase::In) {
            let d = self.read_container().await?;
            if d.container_type == opcode::CONTAINER_RESPONSE {
                return Ok(PtpResponse {
                    code: d.code,
                    params: d.params,
                    data: Vec::new(),
                });
            }
            if d.container_type != opcode::CONTAINER_DATA {
                return Err(Error::UnexpectedContainer {
                    expected: opcode::CONTAINER_DATA,
                    got: d.container_type,
                });
            }
            d.payload
        } else {
            Vec::new()
        };

        // Response container
        let resp = self.read_container().await?;
        if resp.container_type != opcode::CONTAINER_RESPONSE {
            return Err(Error::UnexpectedContainer {
                expected: opcode::CONTAINER_RESPONSE,
                got: resp.container_type,
            });
        }

        Ok(PtpResponse {
            code: resp.code,
            params: resp.params,
            data: in_data,
        })
    }

    pub async fn close(mut self) -> Result<()> {
        if self.session_open {
            let _ = self
                .command(opcode::OP_CLOSE_SESSION, &[], DataPhase::None)
                .await;
            self.session_open = false;
        }
        Ok(())
    }

    async fn bulk_out(&self, data: Vec<u8>) -> Result<()> {
        let completion = self.interface.bulk_out(self.ep_out, data).await;
        completion
            .into_result()
            .map_err(|e| Error::Usb(format!("bulk_out ep=0x{:02X}: {e:?}", self.ep_out)))?;
        Ok(())
    }

    /// Read a single complete PTP container, possibly spanning multiple bulk packets.
    async fn read_container(&self) -> Result<Decoded> {
        let mut buf: Vec<u8> = Vec::new();
        loop {
            let completion = self
                .interface
                .bulk_in(self.ep_in, RequestBuffer::new(READ_CHUNK))
                .await;
            let chunk = completion
                .into_result()
                .map_err(|e| Error::Usb(format!("bulk_in ep=0x{:02X}: {e:?}", self.ep_in)))?;

            if chunk.is_empty() && buf.is_empty() {
                return Err(Error::Usb("empty bulk_in response".into()));
            }
            buf.extend_from_slice(&chunk);

            if buf.len() >= 4 {
                let total = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
                if buf.len() >= total {
                    buf.truncate(total);
                    return container::decode(&buf);
                }
            }
            if chunk.is_empty() {
                return Err(Error::Usb(
                    "short bulk_in chunk before full container".into(),
                ));
            }
        }
    }
}

fn find_ptp_endpoints(config: &nusb::descriptors::Configuration) -> Result<(u8, u8, u8)> {
    for ifgroup in config.interfaces() {
        for alt in ifgroup.alt_settings() {
            // PTP / still-image: class 6, subclass 1, protocol 1.
            if alt.class() == 6 && alt.subclass() == 1 && alt.protocol() == 1 {
                let ifnum = alt.interface_number();
                let mut ep_in = None;
                let mut ep_out = None;
                for ep in alt.endpoints() {
                    if ep.transfer_type() == EndpointType::Bulk {
                        match ep.direction() {
                            Direction::In => ep_in = Some(ep.address()),
                            Direction::Out => ep_out = Some(ep.address()),
                        }
                    }
                }
                if let (Some(i), Some(o)) = (ep_in, ep_out) {
                    return Ok((ifnum, i, o));
                }
            }
        }
    }
    Err(Error::NoPtpInterface)
}
