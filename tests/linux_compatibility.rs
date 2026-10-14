#![cfg(target_os = "linux")]

use sidestep as _;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use upleft_core::document_io::DocumentIO;
use upleft_core::metrics::SentenceTokenizer;

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("omperor-linux-{}-{unique}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn atomic_creates_and_replacements_preserve_external_changes() {
    let scratch = Scratch::new();
    let path = scratch.0.join("document.md");
    DocumentIO::create_atomically(b"original", &path).unwrap();
    assert!(DocumentIO::create_atomically(b"overwrite", &path).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"original");
    DocumentIO::replace_existing_atomically(b"saved", &path, b"original").unwrap();
    std::fs::write(&path, b"external writer").unwrap();
    assert!(DocumentIO::replace_existing_atomically(b"stale save", &path, b"saved").is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"external writer");
}

#[test]
fn sentence_ranges_count_utf16_and_allow_early_stop() {
    let tokenizer = SentenceTokenizer::new();
    let text = "😀 First. Second!";
    let mut ranges = Vec::new();
    tokenizer.enumerate(text, |range| {
        ranges.push(range);
        true
    });
    assert_eq!(ranges.len(), 2);
    assert_eq!(ranges[0].location, 0);
    assert_eq!(
        ranges[1].location as usize,
        "😀 First. ".encode_utf16().count()
    );
    assert_eq!(
        (ranges[1].location + ranges[1].length) as usize,
        text.encode_utf16().count()
    );
    let mut visited = 0;
    tokenizer.enumerate(text, |_| {
        visited += 1;
        false
    });
    assert_eq!(visited, 1);
}

#[test]
fn mermaid_numbers_and_showcase_diagrams_render_on_workers() {
    use upleft_mermaid::{DiagramTheme, LayoutConfig, MermaidImageRenderer, Payload};

    // Swift's numeric grammar includes hexadecimal floats and non-finite
    // values, and always uses the C decimal separator.
    for (text, expected) in [
        ("4", 4.0),
        ("1.5", 1.5),
        ("-2.5", -2.5),
        ("1e-3", 0.001),
        ("0x1.8p+1", 3.0),
    ] {
        assert_eq!(upleft_mermaid::swift::parse_double(text), Some(expected));
    }
    assert!(
        upleft_mermaid::swift::parse_double("inf")
            .unwrap()
            .is_infinite()
    );
    assert!(upleft_mermaid::swift::parse_double("nan").unwrap().is_nan());
    for text in ["", " 1", "1 ", "1,5", "not a number"] {
        assert_eq!(upleft_mermaid::swift::parse_double(text), None);
    }

    let showcase = include_str!("../src/mock_showcase.md");
    let diagrams: Vec<_> = showcase
        .split("```mermaid\n")
        .skip(1)
        .map(|part| part.split("```").next().unwrap())
        .collect();
    assert_eq!(diagrams.len(), 6);
    std::thread::scope(|scope| {
        let workers: Vec<_> = diagrams
            .into_iter()
            .map(|source| {
                scope.spawn(move || {
                    objc2::rc::autoreleasepool(|_| {
                        let graph =
                            upleft_mermaid::parser::parse(source).expect("showcase diagram parses");
                        if let Payload::XyChart(chart) = &graph.payload {
                            assert_eq!(chart.series[0].data, [4.0, 9.0, 16.0]);
                        }
                        let renderer = MermaidImageRenderer::new(
                            DiagramTheme::zinc_light(),
                            LayoutConfig::default(),
                        );
                        let prepared = renderer
                            .prepare(source)
                            .expect("diagram prepares")
                            .expect("diagram has content");
                        let image = upleft_mermaid::downright::mermaid_renderer_bridge::render(
                            &prepared, 2.0,
                        )
                        .expect("showcase diagram renders on a worker");
                        assert!(image.size.width > 0.0 && image.size.height > 0.0);
                    });
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
    });
}
