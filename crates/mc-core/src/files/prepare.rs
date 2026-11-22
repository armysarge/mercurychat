//! Preparing a file to send (M4 C2): a photo is shrunk first, then every
//! file is zstd-compressed when that makes it smaller. Blocking: run it on
//! a blocking task.

use mch_protocol::payload::Encoding;
use std::path::{Path, PathBuf};

/// The long edge a photo is shrunk to; `Original` sends its bytes untouched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum PhotoSize {
    Small,
    #[default]
    Medium,
    Large,
    Original,
}

impl PhotoSize {
    pub fn long_edge(self) -> Option<u32> {
        match self {
            PhotoSize::Small => Some(320),
            PhotoSize::Medium => Some(640),
            PhotoSize::Large => Some(1024),
            PhotoSize::Original => None,
        }
    }
}

/// A file ready to offer: `path` holds the wire bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prepared {
    pub path: PathBuf,
    /// The file as it will land on disk.
    pub name: String,
    pub size: u64,
    pub sha256: String,
    pub wire_size: u64,
    pub encoding: Option<Encoding>,
    pub shrunk: bool,
    /// The photo's own bytes go out and carry EXIF (location, camera, …).
    pub keeps_metadata: bool,
    pub mime: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrepareError {
    TooLarge { size: u64, max: u64 },
    Unreadable(String),
    Io(String),
}

impl std::fmt::Display for PrepareError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PrepareError::TooLarge { size, max } => write!(f, "the file is {size} bytes; the limit is {max}"),
            PrepareError::Unreadable(e) => write!(f, "the file can't be read: {e}"),
            PrepareError::Io(e) => write!(f, "{e}"),
        }
    }
}

/// JPEG quality for a shrunk photo (M4 C2: start at 70).
const JPEG_QUALITY: u8 = 70;
/// zstd level (high: files are small and airtime is dear).
const ZSTD_LEVEL: i32 = 19;
/// A photo bigger than this isn't decoded at all (it would be refused
/// anyway unless shrinking saves it, and decoding it costs memory).
const MAX_PHOTO_SOURCE: u64 = 64 << 20;

/// Shrink (photos) and compress `src` into a new file in `tmp_dir`. `max`
/// applies to the file as it will land on disk.
pub fn prepare(src: &Path, photo: PhotoSize, tmp_dir: &Path, max: u64) -> Result<Prepared, PrepareError> {
    use sha2::{Digest, Sha256};
    let unreadable = |e: std::io::Error| PrepareError::Unreadable(e.to_string());
    let src_size = std::fs::metadata(src).map_err(unreadable)?.len();
    let mut name = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
    let limit = if photo.long_edge().is_some() { max.max(MAX_PHOTO_SOURCE) } else { max };
    if src_size > limit {
        return Err(PrepareError::TooLarge { size: src_size, max });
    }
    let source = std::fs::read(src).map_err(unreadable)?;
    let format = image::guess_format(&source).ok().filter(|f| {
        matches!(
            f,
            image::ImageFormat::Jpeg | image::ImageFormat::Png | image::ImageFormat::WebP | image::ImageFormat::Gif
        )
    });
    let mut mime = format.map(|f| f.to_mime_type().to_string());
    let mut bytes = source;
    let mut shrunk = false;
    let mut keeps_metadata = false;
    if let Some(f) = format {
        let still = f != image::ImageFormat::Gif || !animated_gif(&bytes);
        match photo.long_edge().filter(|_| still).and_then(|edge| shrink(&bytes, f, edge)) {
            Some(jpeg) if jpeg.len() < bytes.len() => {
                bytes = jpeg;
                shrunk = true;
                mime = Some("image/jpeg".into());
                name = jpg_name(&name);
            }
            _ => keeps_metadata = has_exif(&bytes, f),
        }
    }
    let size = bytes.len() as u64;
    if size > max {
        return Err(PrepareError::TooLarge { size, max });
    }
    let sha256 = hex::encode(Sha256::digest(&bytes));
    let packed = zstd::encode_all(bytes.as_slice(), ZSTD_LEVEL).map_err(|e| PrepareError::Io(e.to_string()))?;
    let (wire, encoding) = if packed.len() < bytes.len() { (packed, Some(Encoding::Zstd)) } else { (bytes, None) };
    std::fs::create_dir_all(tmp_dir).map_err(|e| PrepareError::Io(e.to_string()))?;
    let path = tmp_dir.join(format!("{}.prep", mch_protocol::ObjectId::random()));
    std::fs::write(&path, &wire).map_err(|e| PrepareError::Io(e.to_string()))?;
    Ok(Prepared { path, name, size, sha256, wire_size: wire.len() as u64, encoding, shrunk, keeps_metadata, mime })
}

/// Decode, turn upright, fit inside `edge` × `edge` (never enlarging) and
/// encode as baseline JPEG. Re-encoding writes no metadata. `None` when the
/// photo can't be decoded.
fn shrink(bytes: &[u8], format: image::ImageFormat, edge: u32) -> Option<Vec<u8>> {
    use image::{ImageDecoder, ImageEncoder};
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes));
    reader.set_format(format);
    let mut decoder = reader.into_decoder().ok()?;
    let orientation = decoder.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut img = image::DynamicImage::from_decoder(decoder).ok()?;
    img.apply_orientation(orientation);
    if img.width().max(img.height()) > edge {
        img = img.resize(edge, edge, image::imageops::FilterType::Lanczos3);
    }
    let rgb = img.to_rgb8();
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY)
        .write_image(rgb.as_raw(), rgb.width(), rgb.height(), image::ExtendedColorType::Rgb8)
        .ok()?;
    Some(out)
}

fn animated_gif(bytes: &[u8]) -> bool {
    use image::AnimationDecoder;
    image::codecs::gif::GifDecoder::new(std::io::Cursor::new(bytes))
        .map(|d| d.into_frames().take(2).count() > 1)
        .unwrap_or(false)
}

fn has_exif(bytes: &[u8], format: image::ImageFormat) -> bool {
    use image::ImageDecoder;
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes));
    reader.set_format(format);
    reader.into_decoder().ok().and_then(|mut d| d.exif_metadata().ok().flatten()).is_some_and(|e| !e.is_empty())
}

/// `scan.png` → `scan.jpg`; a `.jpg` or `.jpeg` name is kept.
fn jpg_name(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        return name.to_string();
    }
    match name.rfind('.') {
        Some(i) if i > 0 => format!("{}.jpg", &name[..i]),
        _ => format!("{name}.jpg"),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::files::unpack::unpack_zstd;
    use image::{ImageEncoder, RgbImage};
    use sha2::{Digest, Sha256};

    /// Noise that neither JPEG nor zstd can squeeze much (deterministic).
    pub(crate) fn noise(w: u32, h: u32) -> RgbImage {
        let mut x: u32 = 0x1234_5678;
        RgbImage::from_fn(w, h, |_, _| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            image::Rgb([x as u8, (x >> 8) as u8, (x >> 16) as u8])
        })
    }

    /// A smooth gradient: what a photo looks like to a JPEG encoder.
    fn gradient(w: u32, h: u32) -> RgbImage {
        RgbImage::from_fn(w, h, |x, y| image::Rgb([(x * 255 / w) as u8, (y * 255 / h) as u8, 128]))
    }

    fn jpeg(img: &RgbImage, quality: u8) -> Vec<u8> {
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality)
            .write_image(img.as_raw(), img.width(), img.height(), image::ExtendedColorType::Rgb8)
            .unwrap();
        out
    }

    /// An EXIF block (big-endian TIFF) with Orientation = `orientation` and a
    /// GPS IFD holding GPSLatitudeRef "S", inserted after the JPEG's SOI.
    fn with_exif(jpeg: &[u8], orientation: u16) -> Vec<u8> {
        let mut t = Vec::new();
        t.extend_from_slice(b"MM\x00\x2a\x00\x00\x00\x08");
        t.extend_from_slice(&2u16.to_be_bytes());
        t.extend_from_slice(&[0x01, 0x12, 0x00, 0x03, 0, 0, 0, 1]);
        t.extend_from_slice(&orientation.to_be_bytes());
        t.extend_from_slice(&[0, 0]);
        t.extend_from_slice(&[0x88, 0x25, 0x00, 0x04, 0, 0, 0, 1, 0, 0, 0, 38]);
        t.extend_from_slice(&[0, 0, 0, 0]);
        t.extend_from_slice(&1u16.to_be_bytes());
        t.extend_from_slice(&[0x00, 0x01, 0x00, 0x02, 0, 0, 0, 2, b'S', 0, 0, 0]);
        t.extend_from_slice(&[0, 0, 0, 0]);
        let mut app1 = b"Exif\x00\x00".to_vec();
        app1.extend_from_slice(&t);
        let mut out = jpeg[..2].to_vec();
        out.extend_from_slice(&[0xFF, 0xE1]);
        out.extend_from_slice(&((app1.len() + 2) as u16).to_be_bytes());
        out.extend_from_slice(&app1);
        out.extend_from_slice(&jpeg[2..]);
        out
    }

    fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, bytes).unwrap();
        p
    }

    /// The bytes that will land on disk: the wire file, unpacked if encoded.
    fn landed(p: &Prepared, dir: &Path) -> Vec<u8> {
        match p.encoding {
            Some(Encoding::Zstd) => {
                let out = dir.join("landed.bin");
                unpack_zstd(&p.path, &out, p.size).unwrap();
                std::fs::read(out).unwrap()
            }
            None => std::fs::read(&p.path).unwrap(),
        }
    }

    fn has_exif(bytes: &[u8]) -> bool {
        bytes.windows(6).any(|w| w == b"Exif\x00\x00")
    }

    const MAX: u64 = 1 << 20;

    #[test]
    fn shrinks_to_medium_and_strips_gps() {
        let d = tempfile::tempdir().unwrap();
        let src = write(d.path(), "photo.jpg", &with_exif(&jpeg(&gradient(1600, 1200), 95), 1));
        let p = prepare(&src, PhotoSize::Medium, d.path(), MAX).unwrap();
        let out = landed(&p, d.path());
        let img = image::load_from_memory(&out).unwrap();
        assert_eq!((img.width(), img.height()), (640, 480));
        assert!(!has_exif(&out), "no EXIF (so no GPS) in the shrunk photo");
        assert!(p.shrunk && !p.keeps_metadata);
        assert_eq!(p.name, "photo.jpg");
        assert_eq!(p.size, out.len() as u64);
        assert_eq!(p.sha256, hex::encode(Sha256::digest(&out)));
        assert_eq!(p.mime.as_deref(), Some("image/jpeg"));
    }

    #[test]
    fn png_becomes_jpg() {
        let d = tempfile::tempdir().unwrap();
        let img = gradient(1200, 900);
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(img.as_raw(), 1200, 900, image::ExtendedColorType::Rgb8)
            .unwrap();
        let src = write(d.path(), "scan.png", &png);
        let p = prepare(&src, PhotoSize::Small, d.path(), MAX).unwrap();
        assert_eq!(p.name, "scan.jpg");
        let out = image::load_from_memory(&landed(&p, d.path())).unwrap();
        assert_eq!((out.width(), out.height()), (320, 240));
    }

    #[test]
    fn never_enlarges_small_photo() {
        let d = tempfile::tempdir().unwrap();
        let src = write(d.path(), "tiny.jpg", &jpeg(&gradient(100, 80), 80));
        let p = prepare(&src, PhotoSize::Large, d.path(), MAX).unwrap();
        let out = image::load_from_memory(&landed(&p, d.path())).unwrap();
        assert_eq!((out.width(), out.height()), (100, 80));
    }

    #[test]
    fn orientation_applied() {
        let d = tempfile::tempdir().unwrap();
        // Orientation 6: the camera was turned; shown rotated 90° clockwise.
        let src = write(d.path(), "turned.jpg", &with_exif(&jpeg(&gradient(1600, 800), 90), 6));
        let p = prepare(&src, PhotoSize::Medium, d.path(), MAX).unwrap();
        let out = image::load_from_memory(&landed(&p, d.path())).unwrap();
        assert_eq!((out.width(), out.height()), (320, 640), "upright, long edge 640");
    }

    #[test]
    fn not_smaller_sends_the_source() {
        let d = tempfile::tempdir().unwrap();
        let bytes = jpeg(&noise(200, 150), 20);
        let src = write(d.path(), "rough.jpg", &bytes);
        let p = prepare(&src, PhotoSize::Large, d.path(), MAX).unwrap();
        assert!(!p.shrunk);
        assert_eq!(landed(&p, d.path()), bytes);
    }

    #[test]
    fn animated_gif_sent_as_is() {
        let d = tempfile::tempdir().unwrap();
        let mut gif = Vec::new();
        {
            let mut enc = image::codecs::gif::GifEncoder::new(&mut gif);
            for i in 0..2u8 {
                let f = image::RgbaImage::from_pixel(400, 300, image::Rgba([i * 100, 50, 50, 255]));
                enc.encode_frame(image::Frame::new(f)).unwrap();
            }
        }
        let src = write(d.path(), "wave.gif", &gif);
        let p = prepare(&src, PhotoSize::Small, d.path(), MAX).unwrap();
        assert!(!p.shrunk);
        assert_eq!(p.name, "wave.gif");
        assert_eq!(landed(&p, d.path()), gif);
    }

    #[test]
    fn original_keeps_bytes_and_flags_metadata() {
        let d = tempfile::tempdir().unwrap();
        let bytes = with_exif(&jpeg(&gradient(800, 600), 90), 1);
        let src = write(d.path(), "photo.jpg", &bytes);
        let p = prepare(&src, PhotoSize::Original, d.path(), MAX).unwrap();
        assert!(!p.shrunk && p.keeps_metadata);
        assert_eq!(landed(&p, d.path()), bytes);
        // Without EXIF there is nothing to warn about.
        let plain = write(d.path(), "plain.jpg", &jpeg(&gradient(800, 600), 90));
        assert!(!prepare(&plain, PhotoSize::Original, d.path(), MAX).unwrap().keeps_metadata);
    }

    #[test]
    fn text_compresses_with_zstd() {
        let d = tempfile::tempdir().unwrap();
        let text = "CQ CQ de ZS6ABC, QTH Pretoria, rig IC-7300 at 50 W into a dipole.\n".repeat(60);
        let src = write(d.path(), "notes.txt", text.as_bytes());
        let p = prepare(&src, PhotoSize::Medium, d.path(), MAX).unwrap();
        assert_eq!(p.encoding, Some(Encoding::Zstd));
        assert!(p.wire_size * 4 < p.size, "{} of {}", p.wire_size, p.size);
        assert_eq!(p.size, text.len() as u64);
        assert_eq!(landed(&p, d.path()), text.as_bytes());
        assert!(!p.shrunk && !p.keeps_metadata);
        assert_eq!(p.name, "notes.txt");
    }

    #[test]
    fn incompressible_goes_as_it_is() {
        let d = tempfile::tempdir().unwrap();
        // Random bytes: zstd can only add its frame header.
        let bytes = noise(64, 64).into_raw();
        let src = write(d.path(), "random.bin", &bytes);
        let p = prepare(&src, PhotoSize::Medium, d.path(), MAX).unwrap();
        assert_eq!(p.encoding, None, "not smaller, so it goes as it is");
        assert_eq!(p.wire_size, p.size);
        assert_eq!(std::fs::read(&p.path).unwrap(), bytes);
    }

    /// Any saving counts on HF: a JPEG that zstd squeezes by ~1 % is still
    /// sent compressed (G2), and lands byte for byte.
    #[test]
    fn jpeg_compressed_only_when_smaller() {
        let d = tempfile::tempdir().unwrap();
        let bytes = jpeg(&noise(300, 200), 90);
        let src = write(d.path(), "noise.jpg", &bytes);
        let p = prepare(&src, PhotoSize::Original, d.path(), MAX).unwrap();
        assert!(p.wire_size <= p.size);
        assert_eq!(p.encoding.is_some(), p.wire_size < p.size);
        assert_eq!(landed(&p, d.path()), bytes);
    }

    #[test]
    fn too_large_refused() {
        let d = tempfile::tempdir().unwrap();
        let src = write(d.path(), "big.bin", &vec![7u8; 5000]);
        assert_eq!(
            prepare(&src, PhotoSize::Medium, d.path(), 4096),
            Err(PrepareError::TooLarge { size: 5000, max: 4096 })
        );
        assert!(prepare(&d.path().join("missing.txt"), PhotoSize::Medium, d.path(), MAX).is_err());
    }
}
