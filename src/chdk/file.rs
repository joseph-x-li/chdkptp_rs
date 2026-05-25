//! CHDK file transfer: upload, download, and the `TempData` scratch buffer.
//!
//! All three are extension methods on [`PtpSession`].

use super::opcode::{Sub, PTP_OC_CHDK};
use crate::ptp::{DataPhase, PtpSession};
use crate::Result;

impl PtpSession {
    /// Stash a buffer in the camera's scratch area. Used internally as the
    /// filename channel for [`download_file`](Self::download_file), but
    /// exposed for raw use.
    pub async fn chdk_temp_data(&mut self, data: &[u8]) -> Result<()> {
        let resp = self
            .command(
                PTP_OC_CHDK,
                &[Sub::TempData.as_u32(), 0],
                DataPhase::Out(data),
            )
            .await?;
        resp.ok()
    }

    /// Upload a file to the camera's SD card.
    ///
    /// `camera_path` is the camera-side absolute path. The camera's SD card
    /// root is `A/`, so a typical path is `"A/CHDK/SCRIPTS/foo.lua"` or
    /// `"A/DCIM/100CANON/IMG_0001.JPG"`.
    ///
    /// Wire format (data phase OUT): `u32(filename_len_le) | filename_bytes | file_bytes`.
    pub async fn upload_file(&mut self, camera_path: &str, contents: &[u8]) -> Result<()> {
        let path_bytes = camera_path.as_bytes();
        let mut payload = Vec::with_capacity(4 + path_bytes.len() + contents.len());
        payload.extend_from_slice(&(path_bytes.len() as u32).to_le_bytes());
        payload.extend_from_slice(path_bytes);
        payload.extend_from_slice(contents);

        let resp = self
            .command(
                PTP_OC_CHDK,
                &[Sub::UploadFile.as_u32()],
                DataPhase::Out(&payload),
            )
            .await?;
        resp.ok()
    }

    /// Download a file from the camera's SD card.
    ///
    /// Internally: [`chdk_temp_data`](Self::chdk_temp_data) stashes the path,
    /// then `DownloadFile` pulls the bytes via the IN data phase. Returns the
    /// full file as a `Vec<u8>`.
    pub async fn download_file(&mut self, camera_path: &str) -> Result<Vec<u8>> {
        self.chdk_temp_data(camera_path.as_bytes()).await?;
        let resp = self
            .command(
                PTP_OC_CHDK,
                &[Sub::DownloadFile.as_u32()],
                DataPhase::In,
            )
            .await?;
        resp.ok()?;
        Ok(resp.data)
    }
}
