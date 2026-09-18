//! Hand-written little-endian primitives. The repertoire is fixed: `u8`,
//! `u32`/`u64` LE, length-prefixed bytes, length-prefixed UTF-8, and an
//! option flag plus value. Anything else is a second encoding waiting to
//! disagree with the first.

/// What went wrong reading a message off the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// Magic is not `OZVP`.
    BadMagic { got: [u8; 4] },
    /// Fewer than the 13 header bytes arrived.
    TruncatedHeader { len: usize },
    /// The payload is shorter than `payload_len` promised.
    TruncatedPayload { want: usize, got: usize },
    /// A field ran past the payload end.
    Overrun,
    /// A `u8` discriminant names nothing this side knows.
    UnknownOpcode(u8),
    /// A length prefix exceeds the remaining payload (or `u32::MAX` math).
    BadLength(u32),
    /// A UTF-8 field is not valid UTF-8.
    BadUtf8,
    /// An option flag is not 0 or 1.
    BadOptionFlag(u8),
    /// A discriminant or flag names nothing its field knows.
    BadValue {
        /// Which field failed, as written in the DTO.
        field: &'static str,
        /// The byte that failed it.
        value: u8,
    },
}

impl core::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            DecodeError::BadMagic { got } => {
                write!(f, "bad voip ABI magic: {:02x?}, want OZVP", got)
            }
            DecodeError::TruncatedHeader { len } => {
                write!(f, "voip ABI header truncated: {len} bytes")
            }
            DecodeError::TruncatedPayload { want, got } => {
                write!(f, "voip ABI payload truncated: want {want}, got {got}")
            }
            DecodeError::Overrun => write!(f, "voip ABI field ran past the payload end"),
            DecodeError::UnknownOpcode(op) => write!(f, "unknown voip ABI opcode {op:#04x}"),
            DecodeError::BadLength(n) => write!(f, "bad voip ABI length prefix {n}"),
            DecodeError::BadUtf8 => write!(f, "voip ABI UTF-8 field is not valid UTF-8"),
            DecodeError::BadOptionFlag(b) => write!(f, "bad voip ABI option flag {b:#04x}"),
            DecodeError::BadValue { field, value } => {
                write!(f, "bad voip ABI {field}: {value:#04x}")
            }
        }
    }
}

impl std::error::Error for DecodeError {}

/// Appends the fixed repertoire to a byte buffer.
#[derive(Debug, Default)]
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    /// An empty payload buffer.
    pub fn new() -> Self {
        Writer { buf: Vec::new() }
    }

    /// The bytes written so far.
    pub fn bytes(&self) -> &[u8] {
        &self.buf
    }

    /// Consumes the writer into the finished payload.
    pub fn finish(self) -> Vec<u8> {
        self.buf
    }

    /// A single byte: opcodes, kinds, flags, small enums.
    pub fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    /// 32-bit little-endian: handles, lengths, counts, bitrates.
    pub fn u32_le(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    /// 64-bit little-endian: generations, epochs.
    pub fn u64_le(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    /// `u32` length prefix plus the raw bytes.
    pub fn bytes_raw(&mut self, v: &[u8]) {
        self.u32_le(v.len() as u32);
        self.buf.extend_from_slice(v);
    }

    /// `u32` length prefix plus the UTF-8 bytes.
    pub fn utf8(&mut self, v: &str) {
        self.bytes_raw(v.as_bytes());
    }

    /// A 0/1 flag plus the value when present.
    pub fn option(&mut self, v: Option<&[u8]>) {
        match v {
            None => self.u8(0),
            Some(b) => {
                self.u8(1);
                self.bytes_raw(b);
            }
        }
    }

    /// A 0/1 flag plus the UTF-8 value when present.
    pub fn option_str(&mut self, v: Option<&str>) {
        self.option(v.map(str::as_bytes));
    }
}

/// Reads the fixed repertoire back out of a payload slice.
#[derive(Debug)]
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    /// Reads from the start of `buf`.
    pub fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        let end = self.pos.checked_add(n).ok_or(DecodeError::Overrun)?;
        if end > self.buf.len() {
            return Err(DecodeError::Overrun);
        }
        let out = &self.buf[self.pos..end];
        self.pos = end;
        Ok(out)
    }

    /// Bytes consumed so far.
    pub fn consumed(&self) -> usize {
        self.pos
    }

    /// Bytes still unread. Trailing bytes are legal (newer minor fields).
    pub fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }

    /// A single byte.
    pub fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.take(1)?[0])
    }

    /// 32-bit little-endian.
    pub fn u32_le(&mut self) -> Result<u32, DecodeError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// 64-bit little-endian.
    pub fn u64_le(&mut self) -> Result<u64, DecodeError> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    /// `u32` length prefix plus the raw bytes.
    pub fn bytes_raw(&mut self) -> Result<&'a [u8], DecodeError> {
        let len = self.u32_le()?;
        if len as usize > self.remaining() {
            return Err(DecodeError::BadLength(len));
        }
        self.take(len as usize)
    }

    /// `u32` length prefix plus valid UTF-8.
    pub fn utf8(&mut self) -> Result<&'a str, DecodeError> {
        let raw = self.bytes_raw()?;
        core::str::from_utf8(raw).map_err(|_| DecodeError::BadUtf8)
    }

    /// A 0/1 flag plus the value when present.
    pub fn option(&mut self) -> Result<Option<&'a [u8]>, DecodeError> {
        match self.u8()? {
            0 => Ok(None),
            1 => Ok(Some(self.bytes_raw()?)),
            b => Err(DecodeError::BadOptionFlag(b)),
        }
    }

    /// A 0/1 flag plus the UTF-8 value when present.
    pub fn option_str(&mut self) -> Result<Option<&'a str>, DecodeError> {
        match self.option()? {
            None => Ok(None),
            Some(raw) => core::str::from_utf8(raw)
                .map(Option::Some)
                .map_err(|_| DecodeError::BadUtf8),
        }
    }
}
