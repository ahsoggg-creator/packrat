//! Raw DEFLATE in independent blocks (the pigz approach): every block is compressed on its own and ends
//! with a sync-flush marker, so the concatenation of the blocks is one valid DEFLATE stream, and blocks can
//! be compressed on different threads. Only the last block carries the "final" bit.

use flate2::{Compress, Compression, FlushCompress, Status};

pub fn compress_block(input: &[u8], level: u32, last: bool) -> Vec<u8> {
    let mut c = Compress::new(Compression::new(level.min(9)), false);
    let flush = if last {
        FlushCompress::Finish
    } else {
        FlushCompress::Sync
    };
    let mut out: Vec<u8> = Vec::with_capacity(input.len() / 2 + 128);
    loop {
        let consumed = c.total_in() as usize;
        if out.capacity() - out.len() < 1024 {
            out.reserve(input.len() / 4 + 4096);
        }
        let status = c
            .compress_vec(&input[consumed..], &mut out, flush)
            .expect("deflate cannot fail on valid input");
        if status == Status::StreamEnd {
            break;
        }
        // A sync flush is complete once all input is consumed and the output buffer was not the limit.
        if !last && c.total_in() as usize == input.len() && out.len() < out.capacity() {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::read::DeflateDecoder;
    use std::io::Read;

    #[test]
    fn blocks_concatenate_into_one_stream() {
        let data: Vec<u8> = (0..700_000u32).map(|i| (i % 251) as u8 ^ (i >> 9) as u8).collect();
        let chunks: Vec<&[u8]> = data.chunks(200_000).collect();
        let mut packed = Vec::new();
        for (i, c) in chunks.iter().enumerate() {
            packed.extend(compress_block(c, 6, i + 1 == chunks.len()));
        }
        let mut out = Vec::new();
        DeflateDecoder::new(&packed[..]).read_to_end(&mut out).unwrap();
        assert_eq!(out, data);
    }

    #[test]
    fn incompressible_block_still_roundtrips() {
        let mut x = 1u64;
        let data: Vec<u8> = (0..300_000)
            .map(|_| {
                x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                (x >> 33) as u8
            })
            .collect();
        let packed = compress_block(&data, 9, true);
        let mut out = Vec::new();
        DeflateDecoder::new(&packed[..]).read_to_end(&mut out).unwrap();
        assert_eq!(out, data);
    }
}
