//! Save-state serialization.
//!
//! Every component implements a single `transfer` method against the [`Cursor`]
//! trait. A [`WriteCursor`] serializes fields into a byte buffer; a
//! [`ReadCursor`] deserializes them back. Because both directions share one
//! field list per struct, save and load can never drift out of sync.
//!
//! Transient state (the framebuffer, audio ring buffer, serial log) is
//! deliberately not transferred — it is regenerated on the next frame.

pub(crate) trait Cursor {
    fn u8(&mut self, v: &mut u8);
    fn u16(&mut self, v: &mut u16);
    fn u32(&mut self, v: &mut u32);
    fn i32(&mut self, v: &mut i32);
    fn bool(&mut self, v: &mut bool);
    fn usize(&mut self, v: &mut usize);
    fn bytes(&mut self, v: &mut [u8]);
}

pub(crate) struct WriteCursor {
    pub buf: Vec<u8>,
}

impl WriteCursor {
    pub fn new() -> WriteCursor {
        WriteCursor { buf: Vec::new() }
    }
}

impl Cursor for WriteCursor {
    fn u8(&mut self, v: &mut u8) {
        self.buf.push(*v);
    }
    fn u16(&mut self, v: &mut u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    fn u32(&mut self, v: &mut u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    fn i32(&mut self, v: &mut i32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    fn bool(&mut self, v: &mut bool) {
        self.buf.push(*v as u8);
    }
    fn usize(&mut self, v: &mut usize) {
        self.buf.extend_from_slice(&(*v as u32).to_le_bytes());
    }
    fn bytes(&mut self, v: &mut [u8]) {
        self.buf.extend_from_slice(v);
    }
}

pub(crate) struct ReadCursor<'a> {
    buf: &'a [u8],
    pos: usize,
    /// Cleared if a read ran past the end of the buffer (corrupt/short state).
    pub ok: bool,
}

impl<'a> ReadCursor<'a> {
    pub fn new(buf: &'a [u8]) -> ReadCursor<'a> {
        ReadCursor {
            buf,
            pos: 0,
            ok: true,
        }
    }

    fn take(&mut self, n: usize) -> Option<&[u8]> {
        if self.pos + n <= self.buf.len() {
            let s = &self.buf[self.pos..self.pos + n];
            self.pos += n;
            Some(s)
        } else {
            self.ok = false;
            None
        }
    }
}

impl Cursor for ReadCursor<'_> {
    fn u8(&mut self, v: &mut u8) {
        if let Some(s) = self.take(1) {
            *v = s[0];
        }
    }
    fn u16(&mut self, v: &mut u16) {
        if let Some(s) = self.take(2) {
            *v = u16::from_le_bytes([s[0], s[1]]);
        }
    }
    fn u32(&mut self, v: &mut u32) {
        if let Some(s) = self.take(4) {
            *v = u32::from_le_bytes([s[0], s[1], s[2], s[3]]);
        }
    }
    fn i32(&mut self, v: &mut i32) {
        let mut u = 0u32;
        self.u32(&mut u);
        *v = u as i32;
    }
    fn bool(&mut self, v: &mut bool) {
        let mut b = 0u8;
        self.u8(&mut b);
        *v = b != 0;
    }
    fn usize(&mut self, v: &mut usize) {
        let mut u = 0u32;
        self.u32(&mut u);
        *v = u as usize;
    }
    fn bytes(&mut self, v: &mut [u8]) {
        let n = v.len();
        if let Some(s) = self.take(n) {
            v.copy_from_slice(s);
        }
    }
}
