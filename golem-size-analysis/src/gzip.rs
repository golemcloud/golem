use flate2::Compression;
use flate2::write::GzEncoder;
use std::io::Write;

/// The one compression level every Golem size measurement uses, so the collector, the nightly
/// series and the `golem build` size line report the same gzip size for the same file.
pub const GZIP_LEVEL: u32 = 9;

/// The gzip-compressed size of `bytes` at [`GZIP_LEVEL`]. The gzip header carries no file name
/// and a zero modification time, so the size depends only on the input bytes.
pub fn gzip_size(bytes: &[u8]) -> u64 {
    let mut encoder = GzEncoder::new(CountingWriter(0), Compression::new(GZIP_LEVEL));
    encoder
        .write_all(bytes)
        .expect("writing to a counter cannot fail");
    encoder
        .finish()
        .expect("writing to a counter cannot fail")
        .0
}

struct CountingWriter(u64);

impl Write for CountingWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0 += buf.len() as u64;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::read::GzDecoder;
    use std::io::Read;
    use test_r::test;

    #[test]
    fn gzip_size_is_the_length_of_a_decodable_level_9_stream() {
        let input: Vec<u8> = (0..20_000u32)
            .flat_map(|i| (i % 251).to_le_bytes())
            .collect();
        let mut encoder = GzEncoder::new(Vec::new(), Compression::new(9));
        encoder.write_all(&input).unwrap();
        let compressed = encoder.finish().unwrap();
        assert_eq!(gzip_size(&input), compressed.len() as u64);

        let mut decoded = Vec::new();
        GzDecoder::new(compressed.as_slice())
            .read_to_end(&mut decoded)
            .unwrap();
        assert_eq!(decoded, input);

        let mut fast = GzEncoder::new(Vec::new(), Compression::new(1));
        fast.write_all(&input).unwrap();
        assert_ne!(fast.finish().unwrap().len() as u64, gzip_size(&input));
    }
}
