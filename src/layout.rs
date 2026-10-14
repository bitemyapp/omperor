//! Geometry shared by the reading column and composer. Pure calculations make
//! narrow windows and unusually long account/model names testable off AppKit.

pub(crate) const COMPOSER_HEIGHT: f64 = 158.0;
pub(crate) const CHIP_HEIGHT: f64 = 30.0;
pub(crate) const CACHE_BADGE_WIDTH: f64 = 108.0;
/// The header's usage strip moved to `/usage`; the transcript has its room.
pub(crate) const USAGE_STRIP_HEIGHT: f64 = 0.0;

pub(crate) fn composer_input_height(point_size: f64) -> f64 {
    (point_size * 3.65).ceil().max(62.0)
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ReadingColumn {
    pub(crate) x: f64,
    pub(crate) width: f64,
}

pub(crate) fn reading_column(pane_width: f64, measure: f64) -> ReadingColumn {
    let width = (pane_width - 64.0).max(240.0).min(measure + 32.0);
    ReadingColumn {
        x: ((pane_width - width) / 2.0).floor(),
        width,
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Chip {
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) width: f64,
}

/// Compact toolbar when all visible controls fit; two deliberate rows when
/// space is limited. Missing controls take neither width nor a phantom gap.
pub(crate) fn composer_chips(width: f64, natural: [f64; 4], cache_warning: bool) -> [Chip; 4] {
    let gap = 8.0;
    let visible = natural.iter().filter(|&&width| width > 0.0).count();
    let total = natural.iter().sum::<f64>() + gap * visible.saturating_sub(1) as f64;
    let actions = 88.0
        + if cache_warning {
            CACHE_BADGE_WIDTH + 4.0
        } else {
            0.0
        };
    let bottom_room = (width - 32.0 - actions).max(1.0);
    let mut chips = [Chip {
        x: 16.0,
        y: 10.0,
        width: 0.0,
    }; 4];
    if total <= bottom_room {
        let mut x = 16.0;
        for index in [0, 1, 3, 2] {
            chips[index] = Chip {
                x,
                y: 10.0,
                width: natural[index],
            };
            if natural[index] > 0.0 {
                x += natural[index] + gap;
            }
        }
    } else {
        let top_room = (width - 32.0).max(1.0);
        let reasoning = natural[1].min(top_room * 0.44);
        let model = natural[0].min((top_room - reasoning - gap).max(1.0));
        let fast = natural[2].min(bottom_room * 0.4);
        let account = natural[3].min((bottom_room - fast - gap).max(1.0));
        chips[0] = Chip {
            x: 16.0,
            y: 46.0,
            width: model,
        };
        chips[1] = Chip {
            x: 16.0 + model + gap,
            y: 46.0,
            width: reasoning,
        };
        chips[3] = Chip {
            x: 16.0,
            y: 10.0,
            width: account,
        };
        chips[2] = Chip {
            x: 16.0 + if account > 0.0 { account + gap } else { 0.0 },
            y: 10.0,
            width: fast,
        };
    }
    chips
}

pub(crate) fn composer_input_y(chips: &[Chip; 4]) -> f64 {
    chips
        .iter()
        .filter(|chip| chip.width > 0.0)
        .map(|chip| chip.y + CHIP_HEIGHT + 10.0)
        .fold(50.0, f64::max)
}

/// Make room for orchestration while retaining a usable conversation column.
pub(crate) fn activity_width(total: f64, navigation: f64, normal: f64, expanded: bool) -> f64 {
    if expanded {
        560.0_f64.min((total - navigation - 442.0).max(normal))
    } else {
        normal
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expanded_activity_preserves_conversation_room_at_every_window_size() {
        for total in [1080.0, 1200.0, 1440.0, 1920.0] {
            let width = activity_width(total, 248.0, 352.0, true);
            assert!((352.0..=560.0).contains(&width));
            assert!(total - 248.0 - width - 2.0 >= 440.0);
            assert_eq!(activity_width(total, 248.0, 352.0, false), 352.0);
        }
    }

    #[test]
    fn visible_composer_controls_never_overlap_or_cover_actions() {
        for width in [268.0, 320.0, 440.0, 600.0, 752.0, 960.0] {
            for natural in [
                [180.0, 150.0, 60.0, 180.0],
                [900.0; 4],
                [60.0; 4],
                [150.0, 140.0, 60.0, 0.0],
            ] {
                for cache_warning in [false, true] {
                    let chips = composer_chips(width, natural, cache_warning);
                    for (i, chip) in chips
                        .iter()
                        .enumerate()
                        .filter(|(_, chip)| chip.width > 0.0)
                    {
                        let right = width
                            - 16.0
                            - if chip.y == 10.0 {
                                88.0 + if cache_warning {
                                    CACHE_BADGE_WIDTH + 4.0
                                } else {
                                    0.0
                                }
                            } else {
                                0.0
                            };
                        assert!(chip.x + chip.width <= right);
                        for other in chips
                            .iter()
                            .skip(i + 1)
                            .filter(|other| other.width > 0.0 && other.y == chip.y)
                        {
                            assert!(
                                chip.x + chip.width < other.x || other.x + other.width < chip.x
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn missing_account_leaves_no_gap_before_fast() {
        let chips = composer_chips(440.0, [180.0, 150.0, 60.0, 0.0], false);
        assert_eq!(chips[2].x, 16.0);
        let compact = composer_chips(752.0, [180.0, 150.0, 60.0, 0.0], false);
        assert!(compact.iter().all(|chip| chip.y == 10.0));
        assert_eq!(composer_input_y(&compact), 50.0);
    }

    #[test]
    fn reading_column_is_centered_and_capped() {
        for pane in [440.0, 650.0, 1167.0, 1800.0] {
            let column = reading_column(pane, 720.0);
            assert!(column.width <= 752.0);
            assert!(column.x >= 31.0);
            assert!((pane - (2.0 * column.x + column.width)).abs() <= 1.0);
        }
    }
}
