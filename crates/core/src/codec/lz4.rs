//! `lz4_flex` stops after the first frame; parallel output is a sequence of frames, so this reader keeps going
//! until the input ends.

use lz4_flex::frame::FrameDecoder;
use std::io::{self, BufRead, Read};

pub struct Lz4Reader<R: BufRead> {
    dec: Option<FrameDecoder<R>>,
}

impl<R: BufRead> Lz4Reader<R> {
    pub fn new(src: R) -> Self {
        Self {
            dec: Some(FrameDecoder::new(src)),
        }
    }
}

impl<R: BufRead> Read for Lz4Reader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            let Some(dec) = self.dec.as_mut() else { return Ok(0) };
            let n = dec.read(buf)?;
            if n > 0 || buf.is_empty() {
                return Ok(n);
            }
            let mut src = self
                .dec
                .take()
                .map(FrameDecoder::into_inner)
                .expect("decoder is present");
            if src.fill_buf()?.is_empty() {
                return Ok(0);
            }
            self.dec = Some(FrameDecoder::new(src));
        }
    }
}
