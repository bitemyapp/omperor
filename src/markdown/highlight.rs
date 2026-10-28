//! Colouring code in its language: syntect's grammars (two-face's set,
//! which covers TOML, Nix, TypeScript and more), with a theme of our own
//! whose "colours" are Aizome's roles. A token's role is drawn in the
//! current theme's colour for it, so code follows Night and Day without
//! being coloured again.
//!
//! Loading the grammars takes a moment; [`warm_up`] does it on a thread at
//! start so the first code block doesn't wait on it.

use std::str::FromStr;
use std::sync::OnceLock;

use aizome_kit::Role;
use syntect::easy::HighlightLines;
use syntect::highlighting::{Color, ScopeSelectors, StyleModifier, Theme, ThemeItem, ThemeSettings};
use syntect::parsing::{SyntaxReference, SyntaxSet};

use super::Run;

/// Past this many lines a block is shown plain: colouring it all would
/// take longer than reading it.
const MAX_LINES: usize = 4000;
/// Longer lines than this (minified files) are shown plain.
const MAX_LINE: usize = 2000;

/// The roles tokens take, numbered as the theme's red channel carries them.
const ROLES: [Role; 9] =
    [Role::Text, Role::Text3, Role::Success, Role::Warning, Role::Violet, Role::Cyan, Role::Accent, Role::Danger, Role::Text2];

/// Which scopes take which role (by its place in [`ROLES`]).
const SCOPES: &[(&str, u8)] = &[
    ("comment, punctuation.definition.comment", 1),
    ("string, constant.character, punctuation.definition.string, markup.raw", 2),
    ("constant.numeric, constant.language, constant.other, support.constant", 3),
    ("keyword, storage.modifier, storage.type.function, keyword.control, keyword.declaration", 4),
    ("storage.type, entity.name.type, entity.name.class, entity.name.struct, entity.name.enum, support.type, support.class, entity.other.inherited-class", 5),
    ("entity.name.function, support.function, meta.function-call entity.name, variable.function, entity.name.tag, markup.heading", 6),
    ("invalid, markup.deleted, punctuation.definition.deleted", 7),
    ("markup.inserted, punctuation.definition.inserted", 2),
    ("meta.diff.range, meta.diff.header, markup.changed", 5),
    ("variable.parameter, variable.language, keyword.operator, punctuation.separator, entity.other.attribute-name", 8),
];

fn syntaxes() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(two_face::syntax::extra_newlines)
}

fn theme() -> &'static Theme {
    static THEME: OnceLock<Theme> = OnceLock::new();
    THEME.get_or_init(|| {
        let role = |id: u8| Color { r: id, g: 0, b: 0, a: 255 };
        Theme {
            name: Some("Aizome roles".into()),
            author: None,
            settings: ThemeSettings { foreground: Some(role(0)), ..ThemeSettings::default() },
            scopes: SCOPES
                .iter()
                .filter_map(|(selector, id)| {
                    Some(ThemeItem {
                        scope: ScopeSelectors::from_str(selector).ok()?,
                        style: StyleModifier { foreground: Some(role(*id)), background: None, font_style: None },
                    })
                })
                .collect(),
        }
    })
}

/// Load the grammars and the theme now, on the calling thread.
pub(crate) fn warm_up() {
    let _ = (syntaxes(), theme());
}

/// The grammar for a fence's language: by name, by extension, or by a
/// common alias.
fn syntax_for(lang: &str) -> Option<&'static SyntaxReference> {
    let set = syntaxes();
    let lang = lang.trim().to_ascii_lowercase();
    if lang.is_empty() {
        return None;
    }
    let alias = match lang.as_str() {
        "sh" | "shell" | "zsh" | "console" | "shellsession" | "bash" => "bash",
        "rs" => "rust",
        "py" | "python3" => "python",
        "js" | "mjs" | "cjs" => "javascript",
        "ts" | "tsx" | "mts" => "typescript",
        "yml" => "yaml",
        "patch" => "diff",
        "c++" | "cc" | "hpp" => "cpp",
        "golang" => "go",
        "jsonc" | "json5" => "json",
        "md" => "markdown",
        "dockerfile" => "Dockerfile",
        other => other,
    };
    set.find_syntax_by_token(alias)
        .or_else(|| set.find_syntax_by_extension(alias))
        .or_else(|| set.find_syntax_by_name(alias))
}

/// `text`'s lines, coloured as `lang` (plain when the language is unknown
/// or the block too big).
pub(crate) fn lines(lang: &str, text: &str) -> Vec<Vec<Run>> {
    let plain = |line: &str| vec![Run::plain(line.trim_end_matches('\n'))];
    let Some(syntax) = syntax_for(lang) else {
        return text.split('\n').map(plain).collect();
    };
    let set = syntaxes();
    let mut h = HighlightLines::new(syntax, theme());
    let mut out = Vec::new();
    for (index, line) in text.split_inclusive('\n').enumerate() {
        if index >= MAX_LINES || line.len() > MAX_LINE {
            out.push(plain(line));
            continue;
        }
        let Ok(tokens) = h.highlight_line(line, set) else {
            out.push(plain(line));
            continue;
        };
        let mut runs: Vec<Run> = Vec::new();
        for (style, piece) in tokens {
            let piece = piece.trim_end_matches('\n');
            if piece.is_empty() {
                continue;
            }
            let role = ROLES.get(usize::from(style.foreground.r)).copied().filter(|r| *r != Role::Text);
            match runs.last_mut() {
                Some(last) if last.role == role => last.text.push_str(piece),
                _ => runs.push(Run { text: piece.to_owned(), role, ..Run::default() }),
            }
        }
        out.push(runs);
    }
    // `split_inclusive` gives nothing for an empty text, and nothing for
    // the empty line after a final newline.
    if text.is_empty() || text.ends_with('\n') {
        out.push(Vec::new());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn languages_are_found_by_name_extension_and_alias() {
        for lang in ["rust", "rs", "python", "py", "sh", "bash", "toml", "json", "ts", "diff", "nix", "yaml"] {
            assert!(syntax_for(lang).is_some(), "{lang}");
        }
        assert!(syntax_for("").is_none());
        assert!(syntax_for("no-such-language").is_none());
    }

    #[test]
    fn a_diff_marks_what_went_and_came() {
        let lines = lines("diff", "@@ -1 +1 @@\n-old\n+new");
        assert!(lines[1].iter().any(|r| r.role == Some(Role::Danger)), "{lines:?}");
        assert!(lines[2].iter().any(|r| r.role == Some(Role::Success)), "{lines:?}");
    }

    #[test]
    fn unknown_languages_and_empty_text_stay_plain() {
        assert_eq!(lines("klingon", "a\nb"), vec![vec![Run::plain("a")], vec![Run::plain("b")]]);
        assert_eq!(lines("rust", ""), vec![Vec::<Run>::new()]);
    }
}
