//! Estimate corrections held back from the scroll bar.
//!
//! A transcript row shows an estimated height until its view is made, and a
//! diagram, formula or image shows a placeholder until it lands. When the
//! measurement replaces the guess, the document's height changes, and for a
//! row above the reader so does the scroll origin (the text on screen is kept
//! still): the scroll bar's thumb jumps. Instead the difference is held, with
//! the opposite sign, as a *reserve* on rows far from the viewport on the
//! same side that have no view yet, and given back a few device pixels a
//! frame. A reserve above the reader is given back with the scroll origin,
//! so the text stays put while the thumb glides; one below changes only the
//! document's height.
//!
//! This module is the bookkeeping alone: which rows hold how much, what a
//! row can hold, moving a reserve off a row about to be made, and the steps
//! it is given back in. It knows nothing of AppKit.

/// Time constant of the give-back: each frame returns this share of what is
/// left, `1 − e^(−Δt/τ)`.
pub(crate) const TAU: f64 = 0.070;
/// The give-back never goes slower than this, in points a second, so the
/// exponential's tail ends within a few frames.
pub(crate) const MIN_SPEED: f64 = 120.0;
/// A frame's give-back counts at most this long a gap: the first frame
/// after a pause is not a jump.
const MAX_FRAME: f64 = 0.05;

/// The rows as the bookkeeping sees them, by index.
pub(crate) trait Rows {
    /// The row's own height: measured, remembered or estimated.
    fn base(&self, index: usize) -> f64;
    /// What the row holds on top of it; its shown height is the sum.
    fn reserve(&self, index: usize) -> f64;
    fn set_reserve(&mut self, index: usize, reserve: f64);
}

/// The height a row is laid out at: its own and its reserve, never below
/// zero (its own height may have been guessed again under the reserve).
pub(crate) fn shown(base: f64, reserve: f64) -> f64 {
    (base + reserve).max(0.0)
}

/// `amount` on whole device pixels, toward zero.
pub(crate) fn whole_pixels(amount: f64, scale: f64) -> f64 {
    // Nudged away from zero first: 20.999999 device pixels are 21.
    (amount * scale + if amount < 0.0 { -1.0e-6 } else { 1.0e-6 }).trunc() / scale
}

/// Holds `amount` — added to shown heights — in `candidates`, in order: a
/// row can give up its whole shown height (in whole device pixels) and take
/// any amount. Returns what the candidates could not hold.
pub(crate) fn absorb(rows: &mut impl Rows, candidates: &[usize], amount: f64, scale: f64) -> f64 {
    let mut left = amount;
    for &index in candidates {
        if left.abs() < 1.0e-9 {
            return 0.0;
        }
        let reserve = rows.reserve(index);
        let part = if left > 0.0 {
            left
        } else {
            let room = whole_pixels(shown(rows.base(index), reserve), scale);
            left.max(-room)
        };
        if part != 0.0 {
            rows.set_reserve(index, reserve + part);
            left -= part;
        }
    }
    if left.abs() < 1.0e-9 { 0.0 } else { left }
}

/// Moves row `from`'s reserve into `candidates` (the row is about to be
/// made and must show its own height). Returns what they could not hold,
/// which is settled at once.
pub(crate) fn transfer(rows: &mut impl Rows, from: usize, candidates: &[usize], scale: f64) -> f64 {
    let reserve = rows.reserve(from);
    if reserve == 0.0 {
        return 0.0;
    }
    rows.set_reserve(from, 0.0);
    absorb(rows, candidates, reserve, scale)
}

/// The part of `reserve` to give back in a frame `elapsed` seconds after the
/// one before: an exponential share, at least `MIN_SPEED`, in whole device
/// pixels, and all of what is left once that is a pixel or less.
pub(crate) fn bleed_step(reserve: f64, elapsed: f64, scale: f64) -> f64 {
    let size = reserve.abs();
    let pixel = 1.0 / scale;
    if size <= pixel + 1.0e-9 {
        return reserve;
    }
    let elapsed = elapsed.clamp(0.0, MAX_FRAME);
    let share = size * (1.0 - (-elapsed / TAU).exp());
    let step = share.max(MIN_SPEED * elapsed);
    let step = ((step * scale).round() / scale).max(pixel);
    // What would be left under a pixel goes too.
    let step = if size - step <= pixel + 1.0e-9 { size } else { step };
    step.min(size) * reserve.signum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    struct Row {
        base: f64,
        reserve: f64,
    }

    impl Rows for Vec<Row> {
        fn base(&self, index: usize) -> f64 {
            self[index].base
        }
        fn reserve(&self, index: usize) -> f64 {
            self[index].reserve
        }
        fn set_reserve(&mut self, index: usize, reserve: f64) {
            self[index].reserve = reserve;
        }
    }

    fn rows(bases: &[f64]) -> Vec<Row> {
        bases.iter().map(|&base| Row { base, reserve: 0.0 }).collect()
    }

    fn total(rows: &[Row]) -> f64 {
        rows.iter().map(|row| shown(row.base, row.reserve)).sum()
    }

    #[test]
    fn a_growth_is_taken_from_the_first_row_that_can_hold_it() {
        let mut rows = rows(&[100.0, 100.0, 100.0]);
        assert_eq!(absorb(&mut rows, &[0, 1], -40.0, 2.0), 0.0);
        assert_eq!(rows[0].reserve, -40.0);
        assert_eq!(rows[1].reserve, 0.0);
        assert_eq!(total(&rows), 260.0);
    }

    #[test]
    fn a_growth_too_large_for_one_row_spreads_and_never_goes_below_zero() {
        let mut rows = rows(&[30.0, 50.0, 20.0]);
        let left = absorb(&mut rows, &[0, 1, 2], -90.0, 2.0);
        assert_eq!(left, 0.0);
        assert_eq!(rows[0].reserve, -30.0);
        assert_eq!(rows[1].reserve, -50.0);
        assert_eq!(rows[2].reserve, -10.0);
        assert!(rows.iter().all(|row| row.base + row.reserve >= 0.0));
    }

    #[test]
    fn what_no_row_can_hold_is_returned() {
        let mut rows = rows(&[10.0, 5.0]);
        let left = absorb(&mut rows, &[0, 1], -40.0, 2.0);
        assert_eq!(left, -25.0);
        assert_eq!(total(&rows), 0.0);
        let mut none = rows.clone();
        assert_eq!(absorb(&mut none, &[], 12.0, 2.0), 12.0);
    }

    #[test]
    fn a_shrink_is_held_by_one_row_whatever_its_size() {
        let mut rows = rows(&[4.0, 100.0]);
        assert_eq!(absorb(&mut rows, &[0, 1], 300.0, 2.0), 0.0);
        assert_eq!(rows[0].reserve, 300.0);
    }

    #[test]
    fn a_row_gives_up_only_whole_device_pixels() {
        let mut rows = rows(&[10.3, 100.0]);
        absorb(&mut rows, &[0, 1], -20.0, 2.0);
        assert_eq!(rows[0].reserve, -10.0);
        assert_eq!(rows[1].reserve, -10.0);
    }

    #[test]
    fn opposite_corrections_cancel_in_a_row() {
        let mut rows = rows(&[100.0]);
        absorb(&mut rows, &[0], -30.0, 2.0);
        absorb(&mut rows, &[0], 30.0, 2.0);
        assert_eq!(rows[0].reserve, 0.0);
    }

    #[test]
    fn a_row_about_to_be_made_hands_its_reserve_on() {
        let mut rows = rows(&[100.0, 40.0, 100.0]);
        absorb(&mut rows, &[1], -30.0, 2.0);
        let before = total(&rows);
        assert_eq!(transfer(&mut rows, 1, &[0, 2], 2.0), 0.0);
        assert_eq!(rows[1].reserve, 0.0);
        assert_eq!(rows[0].reserve, -30.0);
        assert_eq!(total(&rows), before);
    }

    #[test]
    fn a_reserve_with_nowhere_to_go_is_settled() {
        let mut rows = rows(&[100.0, 40.0]);
        absorb(&mut rows, &[1], -30.0, 2.0);
        assert_eq!(transfer(&mut rows, 1, &[], 2.0), -30.0);
        assert_eq!(rows[1].reserve, 0.0);
        assert_eq!(transfer(&mut rows, 0, &[1], 2.0), 0.0);
    }

    #[test]
    fn the_give_back_is_whole_pixels_and_ends() {
        for (reserve, scale) in [(250.0, 2.0), (-250.0, 2.0), (37.5, 1.0), (-0.5, 2.0), (3.25, 3.0)] {
            let mut left: f64 = reserve;
            let mut frames = 0;
            while left != 0.0 {
                let step = bleed_step(left, 1.0 / 60.0, scale);
                assert_eq!(step.signum(), left.signum());
                assert!(step.abs() <= left.abs() + 1.0e-12);
                if step.abs() < left.abs() {
                    let pixels = step * scale;
                    assert!((pixels - pixels.round()).abs() < 1.0e-9, "{step} at {scale}");
                }
                left -= step;
                if left.abs() < 1.0e-12 {
                    left = 0.0;
                }
                frames += 1;
                assert!(frames < 60, "{reserve} still {left} after {frames} frames");
            }
            // About a quarter of a second at 60 Hz for a large correction.
            assert!(frames <= 24, "{reserve}: {frames} frames");
        }
    }

    #[test]
    fn the_give_back_is_largest_first() {
        let first = bleed_step(200.0, 1.0 / 60.0, 2.0);
        let later = bleed_step(100.0, 1.0 / 60.0, 2.0);
        assert!(first > later && later > 0.0);
        // A long gap is one frame's worth, not all of it.
        assert!(bleed_step(200.0, 2.0, 2.0) < 200.0);
    }

    #[test]
    fn whole_pixels_rounds_toward_zero() {
        assert_eq!(whole_pixels(10.3, 2.0), 10.0);
        assert_eq!(whole_pixels(-10.3, 2.0), -10.0);
        assert_eq!(whole_pixels(10.5, 2.0), 10.5);
        assert_eq!(whole_pixels(0.2, 2.0), 0.0);
    }
}
