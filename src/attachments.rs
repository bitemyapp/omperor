//! Attachment preparation is blocking work: call `prepare` on the job pool, never AppKit.
//! RPC accepts base64 images, not arbitrary file parts. Every attachment also carries
//! an honest local path so binary files and text-only steering remain accessible.
use std::{
    fs::{self, File, OpenOptions},
    io::{Cursor, Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};

const MAX_INLINE_BYTES: usize = 8 * 1024 * 1024;
// Uncompressed pasteboard TIFF has a separate source budget from PNG transport.
const MAX_TIFF_SOURCE_BYTES: usize = 64 * 1024 * 1024;
const MAX_TEXT_BYTES: usize = 128 * 1024;
static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub(crate) enum AttachmentSource {
    File(PathBuf),
    /// Owned bytes copied from the pasteboard; no AppKit object crosses threads.
    Bytes {
        name: String,
        bytes: Vec<u8>,
    },
}

/// Durable draft reference, never an encoded image or native preview cache.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct SavedAttachment {
    pub(crate) name: String,
    pub(crate) path: Option<PathBuf>,
    #[serde(default)]
    pub(crate) error: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct PreparedAttachment {
    pub(crate) name: String,
    pub(crate) mime_type: String,
    pub(crate) byte_len: u64,
    pub(crate) path: PathBuf,
    pub(crate) preview: Option<AttachmentPreview>,
    delivery: Delivery,
}

/// Worker-decoded, tightly packed RGBA8 pixels (straight alpha, row-major).
/// At most 320 × 240 pixels for a 160 × 120 point, 2× display; never upscaled.
#[derive(Debug, Clone)]
pub(crate) struct AttachmentPreview {
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<[u8]>,
}

#[derive(Debug, Clone)]
enum Delivery {
    Image(Value),
    Text(String),
    Path,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct PreparedPrompt {
    pub(crate) message: String,
    pub(crate) images: Vec<Value>,
}

impl From<String> for PreparedPrompt {
    fn from(message: String) -> Self {
        Self {
            message,
            images: Vec::new(),
        }
    }
}

impl From<&str> for PreparedPrompt {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}

impl PreparedPrompt {
    pub(crate) fn payload_bytes(&self) -> usize {
        self.message.len()
            + self
                .images
                .iter()
                .filter_map(|image| image.get("data")?.as_str())
                .map(str::len)
                .sum::<usize>()
    }
}

/// No disk access or encoding. Inline payloads were prepared by the worker.
pub(crate) fn compose_prompt(
    mut text: String,
    attachments: &[PreparedAttachment],
) -> PreparedPrompt {
    let mut images = Vec::new();
    for attachment in attachments {
        // JSON quoting keeps filenames containing newlines or quotes unambiguous.
        let path = json!(attachment.path.to_string_lossy());
        text.push_str(&format!(
            "\n\nAttached file: {}\nLocal path: {path}\nMIME: {}; {} bytes.\n",
            json!(attachment.name),
            attachment.mime_type,
            attachment.byte_len
        ));
        match &attachment.delivery {
            Delivery::Image(image) => {
                text.push_str("Image supplied with this prompt when supported; otherwise read the local path.\n");
                images.push(image.clone());
            }
            Delivery::Text(content) => {
                text.push_str("UTF-8 file content (JSON string):\n");
                text.push_str(&json!(content).to_string());
            }
            Delivery::Path => text
                .push_str("Contents are not inlined. Read the local path to inspect this file.\n"),
        }
    }
    PreparedPrompt {
        message: text,
        images,
    }
}

/// Performs filesystem access, sniffing and encoding on the caller's worker.
/// Large non-image files remain path attachments. Image size and conversion failures
/// are errors rather than silently removing the image from the prompt.
pub(crate) fn prepare(
    source: AttachmentSource,
    storage_dir: &Path,
) -> Result<PreparedAttachment, String> {
    prepare_inner(source, storage_dir).map_err(|error| error.to_string())
}

fn prepare_inner(
    source: AttachmentSource,
    storage_dir: &Path,
) -> std::io::Result<PreparedAttachment> {
    let (path, name) = match source {
        AttachmentSource::File(path) => {
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            (fs::canonicalize(path)?, name)
        }
        AttachmentSource::Bytes { name, bytes } => {
            fs::create_dir_all(storage_dir)?;
            let directory = fs::canonicalize(storage_dir)?;
            let safe_name = Path::new(&name)
                .file_name()
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| std::ffi::OsStr::new("clipboard"));
            let path = loop {
                let serial = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
                let path = directory.join(format!(
                    "{}-{serial}-{}",
                    std::process::id(),
                    safe_name.to_string_lossy()
                ));
                let mut options = OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                match options.open(&path) {
                    Ok(mut file) => {
                        if let Err(error) = file.write_all(&bytes) {
                            let _ = fs::remove_file(&path);
                            return Err(error);
                        }
                        break path;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(error) => return Err(error),
                }
            };
            (path, name)
        }
    };
    if !fs::metadata(&path)?.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Attachments must be regular files",
        ));
    }
    let mut file = File::open(&path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Attachments must be regular files",
        ));
    }
    // Sniff before selecting a budget, then bound the read even if the file grows.
    let mut bytes = Vec::new();
    (&mut file).take(16).read_to_end(&mut bytes)?;
    let is_tiff = sniff_mime(&bytes) == "image/tiff";
    let source_limit = if is_tiff {
        MAX_TIFF_SOURCE_BYTES
    } else {
        MAX_INLINE_BYTES
    };
    (&mut file)
        .take((source_limit + 1 - bytes.len()) as u64)
        .read_to_end(&mut bytes)?;
    let mime_type = sniff_mime(&bytes).to_owned();
    let mut preview = None;
    let delivery = if bytes.len() > source_limit {
        if mime_type.starts_with("image/") {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                if is_tiff {
                    "TIFF source exceeds the 64 MiB source limit"
                } else {
                    "Image source exceeds the 8 MiB attachment limit"
                },
            ));
        }
        Delivery::Path
    } else if mime_type.starts_with("image/") {
        let format = match mime_type.as_str() {
            "image/png" => image::ImageFormat::Png,
            "image/jpeg" => image::ImageFormat::Jpeg,
            "image/gif" => image::ImageFormat::Gif,
            "image/webp" => image::ImageFormat::WebP,
            "image/tiff" => image::ImageFormat::Tiff,
            _ => unreachable!("sniff_mime only returns supported image formats"),
        };
        let decoded = decode_image(&bytes, format)?;
        // Encode full-resolution TIFF before consuming the same decode for its preview.
        let png = if is_tiff {
            Some(convert_tiff(&decoded)?)
        } else {
            None
        };
        preview = Some(make_preview(decoded));
        Delivery::Image(json!({
            "type": "image",
            "mimeType": if is_tiff { "image/png" } else { &mime_type },
            "data": STANDARD.encode(png.as_deref().unwrap_or(&bytes)),
        }))
    } else if mime_type == "text/plain" && bytes.len() <= MAX_TEXT_BYTES {
        Delivery::Text(String::from_utf8(bytes).expect("sniffed UTF-8"))
    } else {
        Delivery::Path
    };
    Ok(PreparedAttachment {
        name,
        mime_type,
        byte_len: metadata.len(),
        path,
        preview,
        delivery,
    })
}

fn decode_image(bytes: &[u8], format: image::ImageFormat) -> std::io::Result<image::DynamicImage> {
    let mut reader = image::ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    reader.decode().map_err(|error| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "{} decode failed: {error}",
                if format == image::ImageFormat::Tiff {
                    "TIFF"
                } else {
                    "Image"
                }
            ),
        )
    })
}

fn make_preview(image: image::DynamicImage) -> AttachmentPreview {
    let (width, height) = (image.width(), image.height());
    let rgba = if width <= 320 && height <= 240 {
        // Keep tiny images intact; the UI can fit them without invented pixels.
        image.into_rgba8()
    } else {
        // Crop first, avoiding a huge intermediate resize for extreme aspect ratios.
        let (crop_width, crop_height) = if u64::from(width) * 3 > u64::from(height) * 4 {
            ((height * 4 / 3).max(1), height)
        } else {
            (width, (width * 3 / 4).max(1))
        };
        let cropped = image.crop_imm(
            (width - crop_width) / 2,
            (height - crop_height) / 2,
            crop_width,
            crop_height,
        );
        cropped
            .resize_exact(
                crop_width.min(320),
                crop_height.min(240),
                image::imageops::FilterType::Triangle,
            )
            .into_rgba8()
    };
    AttachmentPreview {
        width: rgba.width(),
        height: rgba.height(),
        rgba: rgba.into_raw().into(),
    }
}

fn convert_tiff(image: &image::DynamicImage) -> std::io::Result<Vec<u8>> {
    let mut png = Cursor::new(Vec::new());
    image
        .write_to(&mut png, image::ImageFormat::Png)
        .map_err(|error| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("TIFF to PNG encoding failed: {error}"),
            )
        })?;
    let png = png.into_inner();
    if png.len() > MAX_INLINE_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Converted TIFF PNG exceeds the 8 MiB attachment limit",
        ));
    }
    Ok(png)
}

fn sniff_mime(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        "image/jpeg"
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        "image/gif"
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        "image/webp"
    } else if bytes.starts_with(b"II\x2a\0") || bytes.starts_with(b"MM\0\x2a") {
        "image/tiff"
    } else if bytes.starts_with(b"%PDF-") {
        "application/pdf"
    } else if bytes.starts_with(b"PK\x03\x04") {
        "application/zip"
    } else if std::str::from_utf8(bytes).is_ok()
        && !bytes
            .iter()
            .any(|byte| *byte < 0x20 && !matches!(*byte, b'\n' | b'\r' | b'\t'))
    {
        "text/plain"
    } else {
        "application/octet-stream"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(format: image::ImageFormat, width: u32, height: u32) -> Vec<u8> {
        let image = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            width,
            height,
            image::Rgb([23, 101, 207]),
        ));
        let mut bytes = Cursor::new(Vec::new());
        image.write_to(&mut bytes, format).unwrap();
        bytes.into_inner()
    }

    fn storage() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "omperor-attachments-test-{}-{}",
            std::process::id(),
            NEXT_FILE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        fs::canonicalize(path).unwrap()
    }

    #[test]
    fn all_formats_prepare_identical_file_and_bytes_previews() {
        let dir = storage();
        for format in [
            image::ImageFormat::Png,
            image::ImageFormat::Jpeg,
            image::ImageFormat::Gif,
            image::ImageFormat::WebP,
            image::ImageFormat::Tiff,
        ] {
            let bytes = fixture(format, 800, 600);
            let attachment = prepare(
                AttachmentSource::Bytes {
                    name: "misleading.bin".into(),
                    bytes: bytes.clone(),
                },
                &dir,
            )
            .unwrap();
            let from_file = prepare(AttachmentSource::File(attachment.path.clone()), &dir).unwrap();
            let preview = attachment.preview.as_ref().unwrap();
            assert_eq!((preview.width, preview.height), (320, 240), "{format:?}");
            assert_eq!(preview.rgba.len(), 320 * 240 * 4);
            assert_eq!(preview.rgba, from_file.preview.unwrap().rgba);
            assert!(Arc::ptr_eq(&preview.rgba, &preview.clone().rgba));
            assert!(preview.rgba.chunks_exact(4).all(|pixel| pixel[3] == 255));
            let prompt = compose_prompt(String::new(), &[attachment]);
            let transport = STANDARD
                .decode(prompt.images[0]["data"].as_str().unwrap())
                .unwrap();
            if format != image::ImageFormat::Tiff {
                assert_eq!(transport, bytes);
            } else {
                let full = image::load_from_memory(&transport).unwrap();
                assert_eq!((full.width(), full.height()), (800, 600));
            }
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn preview_center_crops_and_preserves_tiny_pixels_and_alpha() {
        for (width, height) in [(800, 240), (320, 800)] {
            let mut image =
                image::RgbaImage::from_pixel(width, height, image::Rgba([255, 0, 0, 255]));
            for y in (height - 240) / 2..(height + 240) / 2 {
                for x in (width - 320) / 2..(width + 320) / 2 {
                    image.put_pixel(x, y, image::Rgba([0, 255, 0, 127]));
                }
            }
            let preview = make_preview(image.into());
            assert_eq!((preview.width, preview.height), (320, 240));
            assert!(preview.rgba.chunks_exact(4).all(|p| p == [0, 255, 0, 127]));
        }
        for (width, height, expected) in [
            (2, 3, (2, 3)),
            (1000, 30, (40, 30)),
            (20, 1000, (20, 15)),
            (1, 1000, (1, 1)),
            (1000, 1, (1, 1)),
        ] {
            let image =
                image::RgbaImage::from_pixel(width, height, image::Rgba([23, 101, 207, 42]));
            let preview = make_preview(image.into());
            assert_eq!((preview.width, preview.height), expected);
            assert!(
                preview
                    .rgba
                    .chunks_exact(4)
                    .all(|p| p == [23, 101, 207, 42])
            );
        }
    }

    #[test]
    fn malformed_images_and_decode_dimension_limits_fail() {
        let dir = storage();
        for format in [
            image::ImageFormat::Png,
            image::ImageFormat::Jpeg,
            image::ImageFormat::Gif,
            image::ImageFormat::WebP,
            image::ImageFormat::Tiff,
        ] {
            let mut bytes = fixture(format, 2, 2);
            bytes.truncate(16);
            let error = prepare(
                AttachmentSource::Bytes {
                    name: "broken".into(),
                    bytes,
                },
                &dir,
            )
            .unwrap_err();
            assert!(error.contains("decode failed"), "{format:?}: {error}");
        }
        for (width, height) in [(16_385, 1), (1, 16_385), (5000, 5000)] {
            let bytes = fixture(image::ImageFormat::Png, width, height);
            let error = prepare(
                AttachmentSource::Bytes {
                    name: "large.png".into(),
                    bytes,
                },
                &dir,
            )
            .unwrap_err();
            assert!(error.contains("decode failed"), "{error}");
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn file_and_clipboard_delivery_contract() {
        let dir = storage();
        let text = prepare(
            AttachmentSource::Bytes {
                name: "../../notes.txt".into(),
                bytes: b"hello\nworld".to_vec(),
            },
            &dir,
        )
        .unwrap();
        assert!(text.preview.is_none());
        assert!(text.path.starts_with(&dir));
        let binary = prepare(
            AttachmentSource::Bytes {
                name: "unknown.png".into(),
                bytes: vec![0, 255, 42],
            },
            &dir,
        )
        .unwrap();
        assert_eq!(binary.mime_type, "application/octet-stream");
        assert!(binary.preview.is_none());
        let png_bytes = fixture(image::ImageFormat::Png, 2, 2);
        let png = prepare(
            AttachmentSource::Bytes {
                name: "image.dat".into(),
                bytes: png_bytes.clone(),
            },
            &dir,
        )
        .unwrap();
        let prompt = compose_prompt("Review".into(), &[text, binary.clone(), png]);
        assert_eq!(prompt.images.len(), 1);
        assert_eq!(
            STANDARD
                .decode(prompt.images[0]["data"].as_str().unwrap())
                .unwrap(),
            png_bytes
        );
        assert_eq!(prompt.images[0]["mimeType"], "image/png");
        assert!(prompt.message.contains("hello\\nworld"));
        assert!(prompt.message.contains(binary.path.to_str().unwrap()));
        assert!(prompt.message.contains("Contents are not inlined"));
        let again = prepare(AttachmentSource::File(binary.path), &dir).unwrap();
        assert_eq!(again.byte_len, 3);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn large_files_are_not_truncated_inline_and_missing_files_fail() {
        let dir = storage();
        let path = dir.join("large.txt");
        File::create(&path)
            .unwrap()
            .set_len((MAX_INLINE_BYTES + 100) as u64)
            .unwrap();
        let attachment = prepare(AttachmentSource::File(path), &dir).unwrap();
        assert!(matches!(attachment.delivery, Delivery::Path));
        assert_eq!(attachment.byte_len, (MAX_INLINE_BYTES + 100) as u64);
        assert!(attachment.preview.is_none());
        assert!(prepare(AttachmentSource::File(dir.join("missing")), &dir).is_err());
        assert!(prepare(AttachmentSource::File(dir.clone()), &dir).is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn tiff_clipboard_is_converted_to_png() {
        let mut tiff = Cursor::new(Vec::new());
        image::DynamicImage::new_rgba8(2560, 1440)
            .write_to(&mut tiff, image::ImageFormat::Tiff)
            .unwrap();
        assert!(tiff.get_ref().len() > MAX_INLINE_BYTES);
        let source_len = tiff.get_ref().len();
        let dir = storage();
        let attachment = prepare(
            AttachmentSource::Bytes {
                name: "clipboard.tiff".into(),
                bytes: tiff.into_inner(),
            },
            &dir,
        )
        .unwrap();
        assert_eq!(attachment.mime_type, "image/tiff");
        let from_file = prepare(AttachmentSource::File(attachment.path.clone()), &dir).unwrap();
        assert!(matches!(from_file.delivery, Delivery::Image(_)));
        let prompt = compose_prompt(String::new(), &[attachment]);
        assert_eq!(prompt.images[0]["mimeType"], "image/png");
        let png = STANDARD
            .decode(prompt.images[0]["data"].as_str().unwrap())
            .unwrap();
        let decoded = image::load_from_memory_with_format(&png, image::ImageFormat::Png).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (2560, 1440));
        assert!(png.len() < 128 * 1024);
        eprintln!(
            "2560x1440 RGBA TIFF {source_len} bytes -> PNG {} bytes",
            png.len()
        );
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn oversized_images_fail_before_conversion_instead_of_becoming_paths() {
        let dir = storage();
        for (name, signature) in [
            ("large.png", &b"\x89PNG\r\n\x1a\n"[..]),
            ("large.jpg", &b"\xff\xd8\xff"[..]),
            ("large.gif", &b"GIF89a"[..]),
            ("large.webp", &b"RIFF\0\0\0\0WEBP"[..]),
        ] {
            let path = dir.join(name);
            let mut file = File::create(&path).unwrap();
            file.write_all(signature).unwrap();
            file.set_len((MAX_INLINE_BYTES + 1) as u64).unwrap();
            let error = prepare(AttachmentSource::File(path), &dir).unwrap_err();
            assert!(error.contains("8 MiB"), "{name}: {error}");
        }
        let mut bytes = vec![0; MAX_TIFF_SOURCE_BYTES + 1];
        bytes[..4].copy_from_slice(b"II\x2a\0");
        let error = prepare(
            AttachmentSource::Bytes {
                name: "clipboard.tiff".into(),
                bytes,
            },
            &dir,
        )
        .unwrap_err();
        assert!(error.contains("TIFF source exceeds the 64 MiB"), "{error}");
        let path = dir.join("huge.tiff");
        let mut file = File::create(&path).unwrap();
        file.write_all(b"II\x2a\0").unwrap();
        file.set_len((MAX_TIFF_SOURCE_BYTES + 1) as u64).unwrap();
        let error = prepare(AttachmentSource::File(path), &dir).unwrap_err();
        assert!(error.contains("TIFF source exceeds the 64 MiB"), "{error}");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn malformed_tiff_reports_conversion_error() {
        let dir = storage();
        let error = prepare(
            AttachmentSource::Bytes {
                name: "broken.tiff".into(),
                bytes: b"II\x2a\0".to_vec(),
            },
            &dir,
        )
        .unwrap_err();
        assert!(error.contains("TIFF decode failed"), "{error}");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn tiff_at_source_limit_still_converts() {
        let mut tiff = Cursor::new(Vec::new());
        image::DynamicImage::new_rgba8(2, 2)
            .write_to(&mut tiff, image::ImageFormat::Tiff)
            .unwrap();
        let mut bytes = tiff.into_inner();
        bytes.resize(MAX_TIFF_SOURCE_BYTES, 0);
        let dir = storage();
        let attachment = prepare(
            AttachmentSource::Bytes {
                name: "boundary.tiff".into(),
                bytes,
            },
            &dir,
        )
        .unwrap();
        assert!(matches!(attachment.delivery, Delivery::Image(_)));
        fs::remove_dir_all(dir).unwrap();
    }
}
