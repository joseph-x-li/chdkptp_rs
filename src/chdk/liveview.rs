//! CHDK live view: `PTP_CHDK_GetDisplayData` + on-host YUV decode.
//!
//! The camera returns a packed buffer containing:
//!   - an `lv_data_header` with offsets to viewport/bitmap/palette descriptors
//!   - the actual framebuffer bytes at those offsets
//!
//! The viewport ("what the camera sees") is typically YUV 4:2:2 packed.
//! Decoders below convert it to RGB888 using BT.601.

use super::opcode::{Sub, PTP_OC_CHDK};
use crate::ptp::{DataPhase, PtpSession};
use crate::{Error, Result};

// --- Flag bits (Param2 of GetDisplayData) ---

/// Request the viewport (sensor preview) framebuffer.
pub const LV_TFR_VIEWPORT: u32 = 0x01;
/// Request the palette (needed to decode a paletted bitmap).
pub const LV_TFR_PALETTE: u32 = 0x02;
/// Request the bitmap (UI overlay) framebuffer.
pub const LV_TFR_BITMAP: u32 = 0x04;
/// Request the bitmap-opacity plane (per-pixel alpha for the overlay).
pub const LV_TFR_BITMAP_OPACITY: u32 = 0x08;

// --- Framebuffer type codes (`fb_type` field of `lv_framebuffer_desc`) ---
// Order matches CHDK's `enum lv_fb_type` in `core/live_view.h` — YUV8 is the
// canonical first variant (= 0), not "no data" as one might assume.

pub const LV_FB_YUV8: u32 = 0;
pub const LV_FB_PAL8: u32 = 1;
pub const LV_FB_YUV8B: u32 = 2;
pub const LV_FB_PAL8_OPACITY: u32 = 3;
pub const LV_FB_YUV8C: u32 = 4;
pub const LV_FB_OPACITY8: u32 = 5;

/// Header at the start of the data response.
///
/// Field layout matches CHDK's `lv_data_header` v2.x. All fields are
/// little-endian u32.
#[derive(Debug, Clone, Copy)]
pub struct LvDataHeader {
    pub version_major: u32,
    pub version_minor: u32,
    pub lcd_aspect_ratio: u32,
    pub palette_type: u32,
    pub palette_data_start: u32,
    pub vp_desc_start: u32,
    pub bm_desc_start: u32,
    /// Present in v2.1+ (offset 28). Earlier versions: garbage / out-of-bounds.
    pub bm_opacity_desc_start: u32,
}

impl LvDataHeader {
    pub const SIZE: usize = 32;

    fn parse(buf: &[u8]) -> Result<Self> {
        if buf.len() < Self::SIZE {
            return Err(Error::Codec(format!(
                "lv_data_header: need {} bytes, have {}",
                Self::SIZE,
                buf.len()
            )));
        }
        let g = |off: usize| u32::from_le_bytes(buf[off..off + 4].try_into().unwrap());
        Ok(Self {
            version_major: g(0),
            version_minor: g(4),
            lcd_aspect_ratio: g(8),
            palette_type: g(12),
            palette_data_start: g(16),
            vp_desc_start: g(20),
            bm_desc_start: g(24),
            bm_opacity_desc_start: g(28),
        })
    }
}

/// A framebuffer plane descriptor (CHDK's `lv_framebuffer_desc`).
///
/// `data_start` is an offset into the full data buffer; `buffer_width` is
/// the row stride in bytes (may include padding); `visible_width` is the
/// visible pixel count per row.
#[derive(Debug, Clone, Copy)]
pub struct FramebufferDesc {
    pub fb_type: u32,
    pub data_start: u32,
    pub buffer_width: u32,
    pub visible_width: u32,
    pub visible_height: u32,
    pub margin_top: u32,
    pub margin_left: u32,
    pub margin_bot: u32,
    pub margin_right: u32,
}

impl FramebufferDesc {
    pub const SIZE: usize = 36;

    fn parse_at(buf: &[u8], offset: u32) -> Result<Self> {
        let off = offset as usize;
        if buf.len() < off + Self::SIZE {
            return Err(Error::Codec(format!(
                "framebuffer_desc at {off}: need {} bytes, buffer has {}",
                Self::SIZE,
                buf.len()
            )));
        }
        let g = |i: usize| u32::from_le_bytes(buf[off + i..off + i + 4].try_into().unwrap());
        Ok(Self {
            fb_type: g(0),
            data_start: g(4),
            buffer_width: g(8),
            visible_width: g(12),
            visible_height: g(16),
            margin_top: g(20),
            margin_left: g(24),
            margin_bot: g(28),
            margin_right: g(32),
        })
    }
}

/// A complete live view frame: header + raw data + lazily-parsed descriptors.
#[derive(Debug, Clone)]
pub struct LiveViewFrame {
    pub header: LvDataHeader,
    /// The full raw payload from `GetDisplayData`. Plane data is accessed via
    /// offsets in the descriptors.
    pub raw: Vec<u8>,
}

impl LiveViewFrame {
    /// Parse the viewport framebuffer descriptor if present.
    pub fn viewport_desc(&self) -> Result<Option<FramebufferDesc>> {
        if self.header.vp_desc_start == 0 {
            return Ok(None);
        }
        FramebufferDesc::parse_at(&self.raw, self.header.vp_desc_start).map(Some)
    }

    /// Parse the bitmap framebuffer descriptor if present.
    pub fn bitmap_desc(&self) -> Result<Option<FramebufferDesc>> {
        if self.header.bm_desc_start == 0 {
            return Ok(None);
        }
        FramebufferDesc::parse_at(&self.raw, self.header.bm_desc_start).map(Some)
    }

    /// Decode the viewport plane to packed RGB888.
    /// Returns `(width, height, rgb_bytes)`. The byte layout is
    /// `[R, G, B, R, G, B, ...]` row-major, no padding.
    ///
    /// The canonical CHDK viewport format is **Y411 / UYVYYY** — 12 bpp,
    /// 6 bytes per 4 pixels: `U Y0 V Y1 Y2 Y3`. One (U, V) chroma pair is
    /// shared by 4 horizontal Y samples. Conversion via BT.601 with U/V
    /// interpreted as signed int8 (CHDK source: `liveimg.c` `yuv_to_r/g/b`).
    pub fn decode_viewport_rgb(&self) -> Result<(u32, u32, Vec<u8>)> {
        let desc = self
            .viewport_desc()?
            .ok_or_else(|| Error::Codec("viewport not present in response".into()))?;

        let width = desc.visible_width as usize;
        let height = desc.visible_height as usize;
        // Y411 row stride: buffer_width is in Y-sample (pixel) units; bytes
        // per row = pixels × 12 / 8 = pixels × 3 / 2.
        let row_bytes = (desc.buffer_width as usize) * 3 / 2;
        let start = desc.data_start as usize;
        let total_needed = start + row_bytes * height;
        if self.raw.len() < total_needed {
            return Err(Error::Codec(format!(
                "viewport plane truncated: need {total_needed} bytes, have {}",
                self.raw.len()
            )));
        }

        match desc.fb_type {
            LV_FB_YUV8 => {
                let mut rgb = Vec::with_capacity(width * height * 3);
                for row in 0..height {
                    let row_start = start + row * row_bytes;
                    let line = &self.raw[row_start..row_start + width * 3 / 2];
                    decode_y411_row(line, width, &mut rgb);
                }
                Ok((width as u32, height as u32, rgb))
            }
            other => Err(Error::Codec(format!(
                "viewport fb_type {other} — only LV_FB_YUV8 (Y411) decoded today"
            ))),
        }
    }
}

/// Decode one row of Y411 / UYVYYY (12 bpp, 6 bytes per 4 pixels) into RGB888.
///
/// Layout per 6 bytes: `U Y0 V Y1 Y2 Y3`. The (U, V) chroma pair is shared
/// across all four Y samples. Conversion uses BT.601 with U/V interpreted as
/// signed int8; coefficients exactly match CHDK's `liveimg.c` `yuv_to_*`
/// functions (fixed-point with 1/4096 scaling).
fn decode_y411_row(line: &[u8], visible_width: usize, out: &mut Vec<u8>) {
    let mut x = 0usize;
    let mut p = 0usize;
    while x + 4 <= visible_width && p + 6 <= line.len() {
        let u = line[p] as i8 as i32;
        let y0 = line[p + 1] as i32;
        let v = line[p + 2] as i8 as i32;
        let y1 = line[p + 3] as i32;
        let y2 = line[p + 4] as i32;
        let y3 = line[p + 5] as i32;

        for &y in &[y0, y1, y2, y3] {
            let (r, g, b) = yuv_to_rgb(y, u, v);
            out.extend_from_slice(&[r, g, b]);
        }
        x += 4;
        p += 6;
    }
}

/// BT.601 YUV → RGB. `y` is 0–255; `u` and `v` are signed (-128..127).
/// Coefficients lifted directly from chdkptp's `liveimg.c` (fixed-point
/// integer scaled by 4096): R += V·5743, G -= U·1411 + V·2925, B += U·7258.
#[inline]
fn yuv_to_rgb(y: i32, u: i32, v: i32) -> (u8, u8, u8) {
    let r = ((y << 12) + v * 5743 + 2048) >> 12;
    let g = ((y << 12) - u * 1411 - v * 2925 + 2048) >> 12;
    let b = ((y << 12) + u * 7258 + 2048) >> 12;
    (
        r.clamp(0, 255) as u8,
        g.clamp(0, 255) as u8,
        b.clamp(0, 255) as u8,
    )
}

impl PtpSession {
    /// Request live view framebuffer data from the camera.
    ///
    /// `flags` is a bitwise OR of `LV_TFR_*` constants. At minimum pass
    /// `LV_TFR_VIEWPORT` to get the sensor preview.
    pub async fn get_display_data(&mut self, flags: u32) -> Result<LiveViewFrame> {
        let resp = self
            .command(
                PTP_OC_CHDK,
                &[Sub::GetDisplayData.as_u32(), flags],
                DataPhase::In,
            )
            .await?;
        resp.ok()?;
        let header = LvDataHeader::parse(&resp.data)?;
        Ok(LiveViewFrame {
            header,
            raw: resp.data,
        })
    }
}
