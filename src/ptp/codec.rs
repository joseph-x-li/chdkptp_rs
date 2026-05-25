//! PTP wire codec — little-endian primitives, length-prefixed UTF-16LE strings, arrays.

use crate::{Error, Result};

pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.remaining() < n {
            return Err(Error::Codec(format!(
                "short read: need {n}, have {}",
                self.remaining()
            )));
        }
        let out = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(out)
    }

    pub fn read_u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    pub fn read_u16(&mut self) -> Result<u16> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn read_u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// PTP string: u8 (UTF-16 char count including the terminating NUL),
    /// followed by `count` little-endian UTF-16 code units (last one is NUL).
    /// A count of 0 means an empty string with no trailing NUL.
    pub fn read_string(&mut self) -> Result<String> {
        let n = self.read_u8()? as usize;
        if n == 0 {
            return Ok(String::new());
        }
        let bytes = self.take(n * 2)?;
        let n_chars = n - 1; // strip terminating NUL
        let mut units = Vec::with_capacity(n_chars);
        for i in 0..n_chars {
            units.push(u16::from_le_bytes([bytes[i * 2], bytes[i * 2 + 1]]));
        }
        String::from_utf16(&units)
            .map_err(|e| Error::Codec(format!("bad UTF-16 in PTP string: {e}")))
    }

    pub fn read_u16_array(&mut self) -> Result<Vec<u16>> {
        let n = self.read_u32()? as usize;
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            out.push(self.read_u16()?);
        }
        Ok(out)
    }
}

#[derive(Default)]
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }

    pub fn write_u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn write_u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn write_bytes(&mut self, b: &[u8]) {
        self.buf.extend_from_slice(b);
    }
}
