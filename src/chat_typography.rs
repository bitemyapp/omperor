//! Persisted point-size overrides. Default sizes follow the selected style
//! and overall scale; explicit sizes are absolute points, never scaled twice.

use crate::theme::{self, ChatStyle};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub(crate) enum TextRole {
    Assistant,
    User,
    Heading1,
    Heading2,
    Heading3,
    SmallHeading,
    InlineCode,
    CodeBlock,
    CodeCaption,
    Speaker,
    Activity,
    Thinking,
    Notice,
    Composer,
    Controls,
    Title,
    Metadata,
}

impl TextRole {
    pub(crate) const ALL: [Self; 17] = [
        Self::Assistant,
        Self::User,
        Self::Heading1,
        Self::Heading2,
        Self::Heading3,
        Self::SmallHeading,
        Self::InlineCode,
        Self::CodeBlock,
        Self::CodeCaption,
        Self::Speaker,
        Self::Activity,
        Self::Thinking,
        Self::Notice,
        Self::Composer,
        Self::Controls,
        Self::Title,
        Self::Metadata,
    ];

    pub(crate) const fn key(self) -> &'static str {
        match self {
            Self::Assistant => "assistant",
            Self::User => "user",
            Self::Heading1 => "heading_1",
            Self::Heading2 => "heading_2",
            Self::Heading3 => "heading_3",
            Self::SmallHeading => "heading_4_6",
            Self::InlineCode => "inline_code",
            Self::CodeBlock => "code_block",
            Self::CodeCaption => "code_caption",
            Self::Speaker => "speaker",
            Self::Activity => "activity",
            Self::Thinking => "thinking",
            Self::Notice => "notice",
            Self::Composer => "composer",
            Self::Controls => "controls",
            Self::Title => "title",
            Self::Metadata => "metadata",
        }
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Assistant => "Assistant replies",
            Self::User => "Your messages",
            Self::Heading1 => "Heading 1",
            Self::Heading2 => "Heading 2",
            Self::Heading3 => "Heading 3",
            Self::SmallHeading => "Headings 4–6",
            Self::InlineCode => "Inline code",
            Self::CodeBlock => "Code blocks",
            Self::CodeCaption => "Code language labels",
            Self::Speaker => "Speaker names",
            Self::Activity => "Tool activity",
            Self::Thinking => "Thinking summaries",
            Self::Notice => "Notices",
            Self::Composer => "Message input",
            Self::Controls => "Model, reasoning & account",
            Self::Title => "Conversation title",
            Self::Metadata => "Status & hints",
        }
    }

    pub(crate) const fn group(self) -> &'static str {
        match self {
            Self::Assistant
            | Self::User
            | Self::Heading1
            | Self::Heading2
            | Self::Heading3
            | Self::SmallHeading
            | Self::InlineCode
            | Self::CodeBlock => "Conversation",
            Self::CodeCaption | Self::Speaker | Self::Activity | Self::Thinking | Self::Notice => {
                "Supporting text"
            }
            _ => "Composer & header",
        }
    }

    pub(crate) const fn heading(level: u8) -> Self {
        match level {
            1 => Self::Heading1,
            2 => Self::Heading2,
            3 => Self::Heading3,
            _ => Self::SmallHeading,
        }
    }

    /// Fixed-height native controls use narrower bounds so their text stays
    /// inside its hit area. The transcript itself has no fixed row heights.
    pub(crate) const fn bounds(self) -> (f64, f64) {
        match self {
            Self::Controls => (10.0, 18.0),
            Self::Title => (14.0, 24.0),
            Self::Metadata => (10.0, 16.0),
            Self::Composer => (10.0, 28.0),
            _ => (10.0, 40.0),
        }
    }

    pub(crate) fn default_size(self, style: ChatStyle, scale: f64) -> f64 {
        let body = match style {
            ChatStyle::Modern => theme::READING_SIZE,
            ChatStyle::Editorial => 15.5,
            ChatStyle::Terminal => 14.0,
        };
        let headings = match style {
            ChatStyle::Modern => [23.0, 20.0, 17.0, 15.0],
            ChatStyle::Editorial => [24.0, 21.0, 18.0, 15.5],
            ChatStyle::Terminal => [20.0, 18.0, 16.0, 14.0],
        };
        match self {
            Self::Assistant | Self::User => body * scale,
            Self::Heading1 => headings[0] * scale,
            Self::Heading2 => headings[1] * scale,
            Self::Heading3 => headings[2] * scale,
            Self::SmallHeading => headings[3] * scale,
            Self::InlineCode | Self::CodeBlock => {
                if style == ChatStyle::Terminal {
                    body * scale
                } else {
                    (body * scale - 1.5).max(11.0)
                }
            }
            Self::CodeCaption => 11.0 * scale,
            Self::Speaker => 12.0 * scale,
            Self::Activity | Self::Thinking => 13.0 * scale,
            Self::Notice => 12.5 * scale,
            Self::Composer => theme::READING_SIZE,
            Self::Controls => theme::CONTROL_SIZE,
            Self::Title => 20.0,
            Self::Metadata => theme::CAPTION_SIZE,
        }
    }
}

/// Half-point units keep equality exact for preference snapshots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TextSizes([Option<u16>; TextRole::ALL.len()]);

impl Default for TextSizes {
    fn default() -> Self {
        Self::DEFAULTS
    }
}

impl TextSizes {
    pub(crate) const DEFAULTS: Self = Self([None; TextRole::ALL.len()]);

    pub(crate) fn get(self, role: TextRole) -> Option<f64> {
        self.0[role as usize].map(|size| f64::from(size) / 2.0)
    }

    /// Reject invalid input, including NaN/infinity and out-of-range values.
    /// Valid numbers snap to the nearest half point for reproducible tuning.
    pub(crate) fn set(&mut self, role: TextRole, points: Option<f64>) -> bool {
        let (min, max) = role.bounds();
        if let Some(size) = points {
            if !size.is_finite() || !(min..=max).contains(&size) {
                return false;
            }
        }
        self.0[role as usize] = points.map(|size| (size * 2.0).round() as u16);
        true
    }

    pub(crate) fn resolve(self, role: TextRole, style: ChatStyle, scale: f64) -> f64 {
        self.get(role)
            .unwrap_or_else(|| role.default_size(style, scale))
    }

    pub(crate) fn from_stored(value: Option<String>) -> Self {
        let mut sizes = Self::DEFAULTS;
        if let Some(value) =
            value.and_then(|value| serde_json::from_str::<serde_json::Value>(&value).ok())
        {
            for role in TextRole::ALL {
                if let Some(points) = value.get(role.key()).and_then(serde_json::Value::as_f64) {
                    sizes.set(role, Some(points));
                }
            }
        }
        sizes
    }

    pub(crate) fn stored(self) -> String {
        let mut values = serde_json::Map::new();
        for role in TextRole::ALL {
            if let Some(points) = self.get(role) {
                values.insert(role.key().into(), points.into());
            }
        }
        serde_json::Value::Object(values).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overrides_are_independent_absolute_and_resettable() {
        let mut sizes = TextSizes::default();
        assert!(sizes.set(TextRole::User, Some(20.5)));
        for style in ChatStyle::ALL {
            for scale in [0.9, 1.0, 1.12] {
                assert_eq!(sizes.resolve(TextRole::User, style, scale), 20.5);
                assert_eq!(
                    sizes.resolve(TextRole::Assistant, style, scale),
                    TextRole::Assistant.default_size(style, scale)
                );
            }
        }
        assert!(sizes.set(TextRole::User, None));
        assert_eq!(sizes, TextSizes::default());
    }

    #[test]
    fn stored_values_validate_each_role_and_tolerate_future_keys() {
        let sizes = TextSizes::from_stored(Some(
            r#"{"user":20.5,"assistant":999,"activity":"bad","controls":40,"future":19}"#.into(),
        ));
        assert_eq!(sizes.get(TextRole::User), Some(20.5));
        for role in [TextRole::Assistant, TextRole::Activity, TextRole::Controls] {
            assert_eq!(sizes.get(role), None);
        }
        assert_eq!(TextSizes::from_stored(Some(sizes.stored())), sizes);
        assert_eq!(
            TextSizes::from_stored(Some("broken json".into())),
            TextSizes::default()
        );
        let mut sizes = sizes;
        for value in [f64::NAN, f64::INFINITY, -1.0, 9.9, 40.1] {
            assert!(!sizes.set(TextRole::User, Some(value)));
        }
        assert_eq!(sizes.get(TextRole::User), Some(20.5));
    }
}
