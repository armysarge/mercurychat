//! Decoding an incoming file's wire bytes (M4 C3), bounded: the output
//! never grows past the size the offer declared.

use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnpackError {
    /// More output than the offer's `size`: stopped at `size` + 1 bytes.
    TooLong,
    Corrupt(String),
    Io(String),
}

impl std::fmt::Display for UnpackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UnpackError::TooLong => write!(f, "the file decodes to more than its declared size"),
            UnpackError::Corrupt(e) => write!(f, "the file does not decode: {e}"),
            UnpackError::Io(e) => write!(f, "{e}"),
        }
    }
}

/// Decode the zstd stream in `wire` into `out`, writing at most `size` + 1
/// bytes: a stream that decodes to more is refused the moment it does (a
/// decompression bomb never fills the disk). Shorter output is not an error
/// here; the caller checks the length and hash. Blocking.
pub fn unpack_zstd(wire: &Path, out: &Path, size: u64) -> Result<(), UnpackError> {
    use std::io::{Read, Write};
    let io = |e: std::io::Error| UnpackError::Io(e.to_string());
    let src = std::fs::File::open(wire).map_err(io)?;
    let mut dec = zstd::stream::read::Decoder::new(src).map_err(|e| UnpackError::Corrupt(e.to_string()))?;
    let mut dst = std::io::BufWriter::new(std::fs::File::create(out).map_err(io)?);
    let mut buf = vec![0u8; 64 * 1024];
    let mut total: u64 = 0;
    loop {
        let n = match dec.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(UnpackError::Corrupt(e.to_string())),
        };
        let room = (size + 1 - total).min(n as u64) as usize;
        dst.write_all(&buf[..room]).map_err(io)?;
        total += room as u64;
        if total > size {
            dst.flush().map_err(io)?;
            return Err(UnpackError::TooLong);
        }
    }
    dst.flush().map_err(io)?;
    dst.get_ref().sync_all().map_err(io)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zstd(bytes: &[u8]) -> Vec<u8> {
        zstd::encode_all(bytes, 19).unwrap()
    }

    #[test]
    fn round_trip_matches_sha256() {
        let d = tempfile::tempdir().unwrap();
        let text = b"73 de ZS6ABC\n".repeat(100);
        std::fs::write(d.path().join("w"), zstd(&text)).unwrap();
        unpack_zstd(&d.path().join("w"), &d.path().join("o"), text.len() as u64).unwrap();
        assert_eq!(std::fs::read(d.path().join("o")).unwrap(), text);
    }

    #[test]
    fn bomb_stops_at_size_plus_one() {
        let d = tempfile::tempdir().unwrap();
        let wire = zstd(&vec![0u8; 1 << 20]);
        assert!(wire.len() < 200, "a megabyte of zeros is tiny on the wire");
        std::fs::write(d.path().join("w"), wire).unwrap();
        assert_eq!(unpack_zstd(&d.path().join("w"), &d.path().join("o"), 1024), Err(UnpackError::TooLong));
        let written = std::fs::metadata(d.path().join("o")).map(|m| m.len()).unwrap_or(0);
        assert!(written <= 1025, "the output never passed size + 1: {written}");
    }

    #[test]
    fn short_output_is_not_too_long() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("w"), zstd(b"abc")).unwrap();
        // Shorter than declared: unpack succeeds; the caller's length check catches it.
        unpack_zstd(&d.path().join("w"), &d.path().join("o"), 10).unwrap();
        assert_eq!(std::fs::read(d.path().join("o")).unwrap(), b"abc");
    }

    #[test]
    fn corrupt_stream_fails() {
        let d = tempfile::tempdir().unwrap();
        let mut wire = zstd(&b"hello ".repeat(50));
        let n = wire.len();
        wire[n / 2] ^= 0xFF;
        wire.truncate(n - 3);
        std::fs::write(d.path().join("w"), wire).unwrap();
        assert!(matches!(
            unpack_zstd(&d.path().join("w"), &d.path().join("o"), 300),
            Err(UnpackError::Corrupt(_) | UnpackError::TooLong)
        ));
    }
}
