//! First-class study elements: reveal tapes (page-level) and spaced-repetition flashcards
//! (notebook-level). Pure data plus a small, deterministic scheduler; no I/O.

use serde::{Deserialize, Serialize};

/// Maximum number of study tapes accepted on one page.
pub const MAX_STUDY_TAPES_PER_PAGE: usize = 256;
/// Maximum number of flashcards accepted in one notebook.
pub const MAX_FLASHCARDS: usize = 20_000;
/// Maximum characters kept for each side of a flashcard.
pub const MAX_CARD_TEXT_CHARS: usize = 2_000;
/// Default tape colour: opaque accent blue from the design tokens (`#0B5FFF`).
pub const DEFAULT_TAPE_RGBA: u32 = 0x0B5F_FFFF;

const COORD_LIMIT: f32 = 65_536.0;
const MIN_TAPE_SIDE: f32 = 4.0;
const MAX_TAPE_SIDE: f32 = 16_384.0;
const DAY_MS: i64 = 86_400_000;
const RELEARN_DELAY_MS: i64 = 10 * 60 * 1000;
const MAX_INTERVAL_DAYS: u32 = 36_500;
const MIN_EASE: u16 = 1_300;
const MAX_EASE: u16 = 3_500;
const DEFAULT_EASE: u16 = 2_500;

/// A tape laid over page content that hides what is beneath it until it is revealed.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct StudyTapeItem {
    pub id: u64,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// Packed `0xRRGGBBAA` tape colour.
    pub color_rgba: u32,
    /// `true` once the learner has tapped the tape to uncover the content.
    pub revealed: bool,
}

impl StudyTapeItem {
    /// Creates a sanitized, hidden tape.
    #[must_use]
    pub fn new(id: u64, x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            id,
            x,
            y,
            width,
            height,
            color_rgba: DEFAULT_TAPE_RGBA,
            revealed: false,
        }
        .sanitized()
    }

    /// Replaces non-finite values and clamps geometry into safe bounds.
    #[must_use]
    pub fn sanitized(self) -> Self {
        let coord = |v: f32| {
            if v.is_finite() {
                v.clamp(-COORD_LIMIT, COORD_LIMIT)
            } else {
                0.0
            }
        };
        let side = |v: f32| {
            if v.is_finite() {
                v.clamp(MIN_TAPE_SIDE, MAX_TAPE_SIDE)
            } else {
                MIN_TAPE_SIDE
            }
        };
        Self {
            x: coord(self.x),
            y: coord(self.y),
            width: side(self.width),
            height: side(self.height),
            ..self
        }
    }

    /// Whether the point lies inside the tape rectangle.
    #[must_use]
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && py >= self.y && px <= self.x + self.width && py <= self.y + self.height
    }
}

/// Truncates a page's tape list to the cap and sanitizes every entry.
pub fn sanitize_study_tapes(items: &mut Vec<StudyTapeItem>) {
    items.truncate(MAX_STUDY_TAPES_PER_PAGE);
    for item in items.iter_mut() {
        *item = item.sanitized();
    }
}

/// Learner self-assessment after seeing the back of a card.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReviewGrade {
    Again,
    Hard,
    Good,
    Easy,
}

/// SM-2 style schedule state for one flashcard.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewSchedule {
    /// UTC milliseconds when the card is next due.
    pub due_ms: i64,
    pub interval_days: u32,
    /// Ease factor in thousandths (2500 = 2.5x).
    pub ease_permille: u16,
    pub repetitions: u32,
    pub lapses: u32,
}

impl ReviewSchedule {
    /// A new card is due immediately.
    #[must_use]
    pub fn new(now_ms: i64) -> Self {
        Self {
            due_ms: now_ms.max(0),
            interval_days: 0,
            ease_permille: DEFAULT_EASE,
            repetitions: 0,
            lapses: 0,
        }
    }

    /// Clamps every field into its valid range.
    #[must_use]
    pub fn sanitized(self) -> Self {
        Self {
            due_ms: self.due_ms.max(0),
            interval_days: self.interval_days.min(MAX_INTERVAL_DAYS),
            ease_permille: self.ease_permille.clamp(MIN_EASE, MAX_EASE),
            repetitions: self.repetitions,
            lapses: self.lapses,
        }
    }

    /// Returns the schedule after reviewing the card at `now_ms` with `grade`.
    #[must_use]
    pub fn review(self, grade: ReviewGrade, now_ms: i64) -> Self {
        let s = self.sanitized();
        let now = now_ms.max(0);
        let scale = |days: u32, permille: u32| -> u32 {
            let scaled = (u64::from(days) * u64::from(permille) + 500) / 1000;
            u32::try_from(scaled).unwrap_or(MAX_INTERVAL_DAYS)
        };
        let (interval_days, ease, repetitions, lapses) = match grade {
            ReviewGrade::Again => (
                0,
                s.ease_permille.saturating_sub(200).max(MIN_EASE),
                0,
                s.lapses.saturating_add(1),
            ),
            ReviewGrade::Hard => (
                scale(s.interval_days.max(1), 1_200).max(1),
                s.ease_permille.saturating_sub(150).max(MIN_EASE),
                s.repetitions.saturating_add(1),
                s.lapses,
            ),
            ReviewGrade::Good => {
                let next = match s.repetitions {
                    0 => 1,
                    1 => 6,
                    _ => scale(s.interval_days.max(1), u32::from(s.ease_permille)),
                };
                (
                    next,
                    s.ease_permille,
                    s.repetitions.saturating_add(1),
                    s.lapses,
                )
            }
            ReviewGrade::Easy => {
                let next = match s.repetitions {
                    0 => 4,
                    _ => scale(
                        s.interval_days.max(1),
                        u32::from(s.ease_permille).saturating_mul(13) / 10,
                    ),
                };
                (
                    next,
                    s.ease_permille.saturating_add(150).min(MAX_EASE),
                    s.repetitions.saturating_add(1),
                    s.lapses,
                )
            }
        };
        let interval_days = interval_days.min(MAX_INTERVAL_DAYS);
        let delay_ms = if interval_days == 0 {
            RELEARN_DELAY_MS
        } else {
            i64::from(interval_days).saturating_mul(DAY_MS)
        };
        Self {
            due_ms: now.saturating_add(delay_ms),
            interval_days,
            ease_permille: ease,
            repetitions,
            lapses,
        }
    }
}

/// A two-sided study card with spaced-repetition scheduling.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Flashcard {
    pub id: u64,
    pub front: String,
    pub back: String,
    /// 0-based page the card was created from, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_page: Option<u32>,
    pub schedule: ReviewSchedule,
}

fn clean_card_text(raw: &str) -> String {
    raw.chars()
        .filter(|c| !c.is_control() || *c == '\n')
        .take(MAX_CARD_TEXT_CHARS)
        .collect()
}

impl Flashcard {
    /// Creates a sanitized card that is due at `now_ms`.
    #[must_use]
    pub fn new(id: u64, front: &str, back: &str, source_page: Option<u32>, now_ms: i64) -> Self {
        Self {
            id,
            front: clean_card_text(front),
            back: clean_card_text(back),
            source_page,
            schedule: ReviewSchedule::new(now_ms),
        }
    }

    /// Re-applies text and schedule limits (used after deserializing hostile input).
    #[must_use]
    pub fn sanitized(self) -> Self {
        Self {
            front: clean_card_text(&self.front),
            back: clean_card_text(&self.back),
            schedule: self.schedule.sanitized(),
            ..self
        }
    }

    #[must_use]
    pub fn is_due(&self, now_ms: i64) -> bool {
        self.schedule.due_ms <= now_ms
    }
}

/// Truncates the deck to the cap, sanitizes every card and drops duplicate ids.
pub fn sanitize_flashcards(cards: &mut Vec<Flashcard>) {
    let mut seen = std::collections::BTreeSet::new();
    let clean: Vec<Flashcard> = std::mem::take(cards)
        .into_iter()
        .filter(|c| seen.insert(c.id))
        .take(MAX_FLASHCARDS)
        .map(Flashcard::sanitized)
        .collect();
    *cards = clean;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tape_sanitizes_hostile_geometry() {
        let t = StudyTapeItem::new(1, f32::NAN, f32::INFINITY, -50.0, f32::NAN);
        assert_eq!((t.x, t.y), (0.0, 0.0));
        assert_eq!((t.width, t.height), (MIN_TAPE_SIDE, MIN_TAPE_SIDE));
        let huge = StudyTapeItem::new(2, 1e30, -1e30, 1e30, 1e30);
        assert!(huge.x <= COORD_LIMIT && huge.y >= -COORD_LIMIT && huge.width <= MAX_TAPE_SIDE);
    }

    #[test]
    fn tape_hit_testing_uses_its_rectangle() {
        let t = StudyTapeItem::new(1, 10.0, 10.0, 100.0, 20.0);
        assert!(t.contains(50.0, 20.0));
        assert!(!t.contains(5.0, 20.0));
        assert!(!t.contains(50.0, 40.0));
    }

    #[test]
    fn tape_list_is_capped() {
        let mut v: Vec<StudyTapeItem> = (0..MAX_STUDY_TAPES_PER_PAGE as u64 + 10)
            .map(|i| StudyTapeItem::new(i, 0.0, 0.0, 10.0, 10.0))
            .collect();
        sanitize_study_tapes(&mut v);
        assert_eq!(v.len(), MAX_STUDY_TAPES_PER_PAGE);
    }

    #[test]
    fn good_reviews_grow_intervals_and_again_resets_them() {
        let now = 1_000_000;
        let s0 = ReviewSchedule::new(now);
        let s1 = s0.review(ReviewGrade::Good, now);
        let s2 = s1.review(ReviewGrade::Good, now);
        let s3 = s2.review(ReviewGrade::Good, now);
        assert_eq!((s1.interval_days, s2.interval_days), (1, 6));
        assert!(s3.interval_days > s2.interval_days);
        assert_eq!(s3.due_ms, now + i64::from(s3.interval_days) * DAY_MS);

        let lapsed = s3.review(ReviewGrade::Again, now);
        assert_eq!(lapsed.repetitions, 0);
        assert_eq!(lapsed.lapses, 1);
        assert_eq!(lapsed.due_ms, now + RELEARN_DELAY_MS);
        assert!(lapsed.ease_permille < s3.ease_permille);
    }

    #[test]
    fn easy_beats_good_beats_hard_for_a_mature_card() {
        let now = 0;
        let mature = ReviewSchedule {
            due_ms: 0,
            interval_days: 30,
            ease_permille: 2_500,
            repetitions: 5,
            lapses: 0,
        };
        let hard = mature.review(ReviewGrade::Hard, now).interval_days;
        let good = mature.review(ReviewGrade::Good, now).interval_days;
        let easy = mature.review(ReviewGrade::Easy, now).interval_days;
        assert!(hard < good && good < easy);
    }

    #[test]
    fn scheduler_is_bounded_for_hostile_state() {
        let hostile = ReviewSchedule {
            due_ms: i64::MAX,
            interval_days: u32::MAX,
            ease_permille: u16::MAX,
            repetitions: u32::MAX,
            lapses: u32::MAX,
        };
        let next = hostile.review(ReviewGrade::Easy, i64::MAX);
        assert!(next.interval_days <= MAX_INTERVAL_DAYS);
        assert!(next.ease_permille <= MAX_EASE);
        assert_eq!(next.due_ms, i64::MAX);
        let neg = ReviewSchedule::new(-5).review(ReviewGrade::Good, -9);
        assert!(neg.due_ms >= 0);
    }

    #[test]
    fn deck_sanitizer_dedupes_ids_and_trims_text() {
        let long = "x".repeat(MAX_CARD_TEXT_CHARS + 50);
        let mut deck = vec![
            Flashcard::new(1, &long, "back\u{0}", None, 0),
            Flashcard::new(1, "dup", "dup", None, 0),
            Flashcard::new(2, "q", "a", Some(3), 0),
        ];
        sanitize_flashcards(&mut deck);
        assert_eq!(deck.len(), 2);
        assert_eq!(deck[0].front.chars().count(), MAX_CARD_TEXT_CHARS);
        assert_eq!(deck[0].back, "back");
        assert!(deck[1].is_due(0));
    }
}
