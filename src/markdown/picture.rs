//! Pictures in answers: where each comes from, and what a worker made of
//! it. An answer's `![alt](place)` names a file (relative to the session's
//! folder, absolute, under `~/`, or a `file:` URL), a picture the answer
//! holds itself (a `data:` URL), or one on the web, which is read only when
//! Settings allows it or the reader asks for that one. PNG, JPEG, GIF,
//! WebP, BMP, TIFF and ICO show as they are; SVG is drawn as shapes.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use aizome_kit::Image;
use base64::Engine as _;

/// The most of a file or a page read for a picture.
const MOST: u64 = 32 * 1024 * 1024;

/// A picture, as far as it has got.
#[derive(Clone)]
pub(crate) enum Picture {
    /// Being read on a worker.
    Loading,
    Shown(Image),
    /// An SVG, as shapes.
    Drawn(Arc<super::svg::Picture>),
    /// On the web, which Settings keeps answers from reading.
    Blocked,
    /// Why it couldn't be read.
    Failed(String),
}

impl std::fmt::Debug for Picture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Picture::Loading => f.write_str("Loading"),
            Picture::Shown(image) => write!(f, "Shown({:?})", image.size()),
            Picture::Drawn(p) => write!(f, "Drawn({:?})", p.size),
            Picture::Blocked => f.write_str("Blocked"),
            Picture::Failed(why) => write!(f, "Failed({why})"),
        }
    }
}

/// Where a picture comes from.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Source {
    Web(String),
    File(PathBuf),
    /// Held in the answer: a `data:` URL.
    Held(String),
}

impl Source {
    /// Where `place`, as an answer wrote it, points: a relative path from
    /// `base`, the session's folder. No I/O: the view asks it too.
    pub(crate) fn of(place: &str, base: Option<&Path>) -> Source {
        let place = place.trim();
        let lower = place.get(..8).unwrap_or(place).to_ascii_lowercase();
        if lower.starts_with("http://") || lower.starts_with("https://") {
            return Source::Web(place.to_owned());
        }
        if lower.starts_with("data:") {
            return Source::Held(place.to_owned());
        }
        let path = match place.strip_prefix("file://") {
            Some(rest) => rest.strip_prefix("localhost").unwrap_or(rest),
            None => place.split_once('#').map_or(place, |(path, _)| path),
        };
        let path = PathBuf::from(String::from_utf8_lossy(&percent_decoded(path)).into_owned());
        let path = match (path.strip_prefix("~"), std::env::var_os("HOME")) {
            (Ok(rest), Some(home)) => Path::new(&home).join(rest),
            _ => path,
        };
        match base {
            Some(base) if path.is_relative() => Source::File(base.join(path)),
            _ => Source::File(path),
        }
    }

    /// What the transcript keeps the picture under.
    pub(crate) fn key(&self) -> u64 {
        let mut h = DefaultHasher::new();
        self.hash(&mut h);
        h.finish()
    }

    /// What opening it opens: its page or its file (none for one the answer
    /// holds).
    pub(crate) fn link(&self) -> Option<String> {
        match self {
            Source::Web(url) => Some(url.clone()),
            Source::File(path) => Some(format!("file://{}", path.display())),
            Source::Held(_) => None,
        }
    }

    pub(crate) fn is_web(&self) -> bool {
        matches!(self, Source::Web(_))
    }

    /// Read it: a worker's job.
    pub(crate) fn load(&self) -> Picture {
        match self {
            Source::File(path) => {
                let read = std::fs::metadata(path).and_then(|m| {
                    if m.len() > MOST {
                        Err(std::io::Error::other("it's too large to show"))
                    } else {
                        std::fs::read(path)
                    }
                });
                match read {
                    Ok(bytes) => picture(bytes, path.extension().is_some_and(|e| e.eq_ignore_ascii_case("svg"))),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        Picture::Failed(format!("There's no file at {}.", path.display()))
                    }
                    Err(e) => Picture::Failed(format!("{} can't be read: {e}.", path.display())),
                }
            }
            Source::Web(url) => match fetch(url) {
                Ok((bytes, svg)) => picture(bytes, svg),
                Err(e) => Picture::Failed(format!("It couldn't be fetched: {e}.")),
            },
            Source::Held(url) => {
                let Some((kind, data)) = url.get(5..).and_then(|rest| rest.split_once(',')) else {
                    return Picture::Failed("Its data: URL is cut short.".into());
                };
                let bytes = if kind.to_ascii_lowercase().ends_with(";base64") {
                    let data: String = data.chars().filter(|c| !c.is_whitespace()).collect();
                    match base64::engine::general_purpose::STANDARD.decode(percent_decoded(&data)) {
                        Ok(bytes) => bytes,
                        Err(e) => return Picture::Failed(format!("Its data: URL isn't base64: {e}.")),
                    }
                } else {
                    percent_decoded(data)
                };
                picture(bytes, kind.to_ascii_lowercase().starts_with("image/svg"))
            }
        }
    }
}

fn fetch(url: &str) -> Result<(Vec<u8>, bool), ureq::Error> {
    let response = ureq::get(url).call()?;
    let svg = response.headers().get("content-type").and_then(|t| t.to_str().ok()).is_some_and(|t| t.contains("svg"));
    let bytes = response.into_body().with_config().limit(MOST).read_to_vec()?;
    Ok((bytes, svg))
}

/// `bytes` as a picture: an SVG (by its name or what it starts with), or a
/// bitmap whose header the kit reads.
fn picture(bytes: Vec<u8>, svg: bool) -> Picture {
    if svg || super::svg::is_svg(&bytes) {
        return match super::svg::read(&bytes, None) {
            Ok(picture) => Picture::Drawn(Arc::new(picture)),
            Err(e) => Picture::Failed(format!("Its SVG can't be read: {e}.")),
        };
    }
    match Image::decode(bytes) {
        Some(image) if image.pixel_size().0 > 0 => Picture::Shown(image),
        _ => Picture::Failed("It isn't a picture Omperor can show.".into()),
    }
}

/// `text` with its `%XX` escapes decoded.
fn percent_decoded(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (byte, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2 × 1 PNG: one red pixel, one blue.
    fn png() -> Vec<u8> {
        let pixels = image::RgbaImage::from_raw(2, 1, vec![255, 0, 0, 255, 0, 0, 255, 255]).unwrap();
        let mut png = std::io::Cursor::new(Vec::new());
        pixels.write_to(&mut png, image::ImageFormat::Png).unwrap();
        png.into_inner()
    }

    #[test]
    fn places_resolve_from_the_session_s_folder() {
        let base = Path::new("/work/project");
        assert_eq!(Source::of("out/plot.png", Some(base)), Source::File("/work/project/out/plot.png".into()));
        assert_eq!(Source::of("my%20plot.png#top", Some(base)), Source::File("/work/project/my plot.png".into()));
        assert_eq!(Source::of("/tmp/a.png", Some(base)), Source::File("/tmp/a.png".into()));
        assert_eq!(Source::of("file:///tmp/b%20c.png", Some(base)), Source::File("/tmp/b c.png".into()));
        assert!(matches!(Source::of("HTTPS://example.test/a.png", None), Source::Web(_)));
        assert!(matches!(Source::of("data:image/png;base64,AAAA", None), Source::Held(_)));
        assert_ne!(Source::of("a.png", Some(base)).key(), Source::of("a.png", Some(Path::new("/elsewhere"))).key());
    }

    #[test]
    fn pictures_load_from_files_and_from_the_answer_or_say_why_not() {
        let encoded = base64::engine::general_purpose::STANDARD.encode(png());
        let held = Source::of(&format!("data:image/png;base64,{encoded}"), None).load();
        assert!(matches!(&held, Picture::Shown(image) if image.pixel_size() == (2, 1)), "{held:?}");
        let svg = "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='8' height='4'%3E%3Crect width='8' height='4'/%3E%3C/svg%3E";
        assert!(matches!(Source::of(svg, None).load(), Picture::Drawn(p) if p.size.width == 8.0));
        let dir = std::env::temp_dir().join(format!("omperor-picture-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("dot.png"), png()).unwrap();
        std::fs::write(dir.join("notes.txt"), "not a picture").unwrap();
        assert!(matches!(Source::of("dot.png", Some(&dir)).load(), Picture::Shown(_)));
        assert!(matches!(Source::of("gone.png", Some(&dir)).load(), Picture::Failed(why) if why.starts_with("There's no file")));
        assert!(matches!(Source::of("notes.txt", Some(&dir)).load(), Picture::Failed(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
