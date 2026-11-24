//! Study commands: reveal tapes (page-level) and spaced-repetition flashcards (notebook-level).
//!
//! Every mutation goes through the same command registry as ink and text edits, so the UI,
//! CLI and any future automation share one validated path.

use std::sync::Arc;

use drawcraft_doc::{
    Flashcard, MAX_FLASHCARDS, MAX_STUDY_TAPES_PER_PAGE, ReviewGrade, StudyTapeItem,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::commands::{invalid, parse_optional_params, parse_params};
use crate::{EngineError, Session};

const DEFAULT_DUE_LIMIT: usize = 50;
const MAX_DUE_LIMIT: usize = 1_000;

#[derive(Deserialize)]
struct TapeAddParams {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    #[serde(default)]
    color_rgba: Option<u32>,
}

#[derive(Deserialize)]
struct TapeRevealParams {
    id: u64,
    #[serde(default)]
    revealed: Option<bool>,
}

#[derive(Deserialize)]
struct IdParams {
    id: u64,
}

#[derive(Deserialize)]
struct CardAddParams {
    front: String,
    back: String,
    #[serde(default)]
    source_page: Option<u32>,
    #[serde(default)]
    now_ms: Option<i64>,
}

#[derive(Deserialize)]
struct CardReviewParams {
    id: u64,
    grade: ReviewGrade,
    #[serde(default)]
    now_ms: Option<i64>,
}

#[derive(Default, Deserialize)]
struct StudyPrefsParams {
    #[serde(default)]
    left_handed: Option<bool>,
    #[serde(default)]
    finger_drawing: Option<bool>,
    #[serde(default)]
    pdf_dark_mode: Option<bool>,
}

#[derive(Default, Deserialize)]
struct CardDueParams {
    #[serde(default)]
    now_ms: Option<i64>,
    #[serde(default)]
    limit: Option<usize>,
}

fn now_or(explicit: Option<i64>) -> i64 {
    explicit.unwrap_or_else(drawcraft_sync_protocol::now_unix_ms)
}

fn next_tape_id(doc: &drawcraft_doc::Document) -> u64 {
    doc.study_tapes
        .iter()
        .chain(doc.pages.iter().flat_map(|p| p.study_tapes.iter()))
        .map(|t| t.id)
        .max()
        .map_or(1, |m| m.saturating_add(1))
}

fn next_card_id(doc: &drawcraft_doc::Document) -> u64 {
    doc.flashcards
        .iter()
        .map(|c| c.id)
        .max()
        .map_or(1, |m| m.saturating_add(1))
}

/// Replaces the session document, optionally recording an undo step.
fn commit(
    session: &mut Session,
    before: Arc<drawcraft_doc::Document>,
    next: drawcraft_doc::Document,
    undo_label: Option<&str>,
) {
    if let Some(label) = undo_label {
        session.history.record(label, before);
    }
    session.doc = Arc::new(next);
    session.mark_active_dirty();
    session.bump_revision();
}

pub(crate) fn cmd_tape_add(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: TapeAddParams = parse_params(id, params)?;
    if ![p.x, p.y, p.width, p.height].iter().all(|v| v.is_finite()) {
        return Err(invalid(id, "tape geometry must be finite"));
    }
    if p.width < 4.0 || p.height < 4.0 {
        return Err(invalid(id, "tape must be at least 4 page points per side"));
    }
    if session.doc.study_tapes.len() >= MAX_STUDY_TAPES_PER_PAGE {
        return Err(invalid(id, "active page study tape limit reached"));
    }
    session.cancel_live_stroke();
    session.cancel_live_eraser();
    let before = Arc::clone(&session.doc);
    let mut next = (*session.doc).clone();
    let tape_id = next_tape_id(&next);
    let mut tape = StudyTapeItem::new(tape_id, p.x, p.y, p.width, p.height);
    if let Some(color) = p.color_rgba {
        tape.color_rgba = color;
    }
    next.study_tapes.push(tape);
    next.sync_active_to_pages();
    commit(session, before, next, Some("Add Study Tape"));
    Ok(json!({ "id": tape_id }))
}

/// Sets or toggles the revealed state of a tape. Revealing is study progress, not an edit,
/// so it is intentionally not an undo step.
pub(crate) fn cmd_tape_reveal(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: TapeRevealParams = parse_params(id, params)?;
    let before = Arc::clone(&session.doc);
    let mut next = (*session.doc).clone();
    let Some(tape) = next.study_tapes.iter_mut().find(|t| t.id == p.id) else {
        return Err(invalid(id, "no study tape with that id on the active page"));
    };
    let revealed = p.revealed.unwrap_or(!tape.revealed);
    if tape.revealed != revealed {
        tape.revealed = revealed;
        next.sync_active_to_pages();
        commit(session, before, next, None);
    }
    Ok(json!({ "id": p.id, "revealed": revealed }))
}

pub(crate) fn cmd_tape_remove(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: IdParams = parse_params(id, params)?;
    session.cancel_live_stroke();
    session.cancel_live_eraser();
    let before = Arc::clone(&session.doc);
    let mut next = (*session.doc).clone();
    let Some(pos) = next.study_tapes.iter().position(|t| t.id == p.id) else {
        return Err(invalid(id, "no study tape with that id on the active page"));
    };
    next.study_tapes.remove(pos);
    next.sync_active_to_pages();
    commit(session, before, next, Some("Remove Study Tape"));
    Ok(json!({ "id": p.id, "removed": true }))
}

pub(crate) fn cmd_card_add(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: CardAddParams = parse_params(id, params)?;
    if p.front.trim().is_empty() || p.back.trim().is_empty() {
        return Err(invalid(id, "flashcard front and back must not be empty"));
    }
    if session.doc.flashcards.len() >= MAX_FLASHCARDS {
        return Err(invalid(id, "notebook flashcard limit reached"));
    }
    if let Some(page) = p.source_page
        && usize::try_from(page).map_or(true, |idx| idx >= session.doc.page_count())
    {
        return Err(invalid(id, "source_page is out of bounds"));
    }
    let before = Arc::clone(&session.doc);
    let mut next = (*session.doc).clone();
    let card_id = next_card_id(&next);
    next.flashcards.push(Flashcard::new(
        card_id,
        &p.front,
        &p.back,
        p.source_page,
        now_or(p.now_ms),
    ));
    commit(session, before, next, Some("Add Flashcard"));
    Ok(json!({ "id": card_id }))
}

pub(crate) fn cmd_card_remove(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: IdParams = parse_params(id, params)?;
    let before = Arc::clone(&session.doc);
    let mut next = (*session.doc).clone();
    let Some(pos) = next.flashcards.iter().position(|c| c.id == p.id) else {
        return Err(invalid(id, "no flashcard with that id"));
    };
    next.flashcards.remove(pos);
    commit(session, before, next, Some("Remove Flashcard"));
    Ok(json!({ "id": p.id, "removed": true }))
}

/// Applies a review grade. Like tape reveals, this is learning progress and not an undo step.
pub(crate) fn cmd_card_review(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: CardReviewParams = parse_params(id, params)?;
    let now = now_or(p.now_ms);
    let before = Arc::clone(&session.doc);
    let mut next = (*session.doc).clone();
    let Some(card) = next.flashcards.iter_mut().find(|c| c.id == p.id) else {
        return Err(invalid(id, "no flashcard with that id"));
    };
    card.schedule = card.schedule.review(p.grade, now);
    let schedule = card.schedule;
    commit(session, before, next, None);
    Ok(json!({
        "id": p.id,
        "due_ms": schedule.due_ms,
        "interval_days": schedule.interval_days,
        "ease_permille": schedule.ease_permille,
        "repetitions": schedule.repetitions,
        "lapses": schedule.lapses,
    }))
}

/// Updates the study-ergonomics preferences (left-handed layout, Pencil-only vs finger drawing,
/// PDF dark reading). Missing fields keep their value.
pub(crate) fn cmd_study_prefs(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: StudyPrefsParams = parse_params(id, params)?;
    if p.left_handed.is_none() && p.finger_drawing.is_none() && p.pdf_dark_mode.is_none() {
        return Err(invalid(
            id,
            "requires at least one of `left_handed`, `finger_drawing` or `pdf_dark_mode`",
        ));
    }
    if let Some(v) = p.left_handed {
        session.prefs.left_handed = v;
    }
    if let Some(v) = p.finger_drawing {
        session.prefs.finger_drawing = v;
    }
    if let Some(v) = p.pdf_dark_mode {
        session.prefs.pdf_dark_mode = v;
    }
    let _ = session.save_prefs();
    session.bump_revision();
    Ok(json!({
        "left_handed": session.prefs.left_handed,
        "finger_drawing": session.prefs.finger_drawing,
        "pdf_dark_mode": session.prefs.pdf_dark_mode,
    }))
}

/// Read-only query: cards due at `now_ms`, earliest first.
pub(crate) fn cmd_card_due(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: CardDueParams = parse_optional_params(id, params)?;
    let now = now_or(p.now_ms);
    let limit = p.limit.unwrap_or(DEFAULT_DUE_LIMIT).clamp(1, MAX_DUE_LIMIT);
    let mut due: Vec<&Flashcard> = session
        .doc
        .flashcards
        .iter()
        .filter(|c| c.is_due(now))
        .collect();
    due.sort_by_key(|c| (c.schedule.due_ms, c.id));
    let total = due.len();
    let cards: Vec<Value> = due
        .into_iter()
        .take(limit)
        .map(|c| {
            json!({
                "id": c.id,
                "front": c.front,
                "back": c.back,
                "source_page": c.source_page,
                "due_ms": c.schedule.due_ms,
            })
        })
        .collect();
    Ok(json!({ "total_due": total, "cards": cards }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Session;

    fn run(s: &mut Session, id: &str, p: Value) -> Result<Value, EngineError> {
        s.execute(id, &p)
    }

    #[test]
    fn tape_add_is_undoable_and_reveal_is_not() -> Result<(), EngineError> {
        let mut s = Session::default();
        let v = run(
            &mut s,
            "studyTape.add",
            json!({"x": 10.0, "y": 10.0, "width": 80.0, "height": 24.0}),
        )?;
        let tape_id = v["id"].as_u64().unwrap_or(0);
        assert_eq!(s.doc.study_tapes.len(), 1);
        assert_eq!(s.doc.pages[0].study_tapes.len(), 1);

        run(&mut s, "studyTape.reveal", json!({"id": tape_id}))?;
        assert!(s.doc.study_tapes[0].revealed);
        // Undo removes the tape entirely (reveal did not add its own step).
        run(&mut s, "edit.undo", json!({}))?;
        assert!(s.doc.study_tapes.is_empty());
        run(&mut s, "edit.redo", json!({}))?;
        assert_eq!(s.doc.study_tapes.len(), 1);
        Ok(())
    }

    #[test]
    fn tape_commands_reject_hostile_input() {
        let mut s = Session::default();
        for bad in [
            json!({"x": null, "y": 0, "width": 10, "height": 10}),
            json!({"x": 0, "y": 0, "width": 1, "height": 10}),
            json!({"x": 0, "y": 0, "width": 10}),
            json!("nope"),
        ] {
            assert!(run(&mut s, "studyTape.add", bad).is_err());
        }
        assert!(run(&mut s, "studyTape.reveal", json!({"id": 99})).is_err());
        assert!(run(&mut s, "studyTape.remove", json!({"id": 99})).is_err());
        assert!(s.doc.study_tapes.is_empty());
    }

    #[test]
    fn tape_ids_stay_unique_across_pages() -> Result<(), EngineError> {
        let mut s = Session::default();
        let a = run(
            &mut s,
            "studyTape.add",
            json!({"x": 0.0, "y": 0.0, "width": 20.0, "height": 20.0}),
        )?["id"]
            .as_u64();
        run(&mut s, "page.add", json!({}))?;
        let b = run(
            &mut s,
            "studyTape.add",
            json!({"x": 0.0, "y": 0.0, "width": 20.0, "height": 20.0}),
        )?["id"]
            .as_u64();
        assert_ne!(a, b);
        Ok(())
    }

    #[test]
    fn flashcard_lifecycle_review_and_due_queue() -> Result<(), EngineError> {
        let mut s = Session::default();
        let t0 = 1_000_000;
        let id = run(
            &mut s,
            "flashcard.add",
            json!({"front": "2+2", "back": "4", "source_page": 0, "now_ms": t0}),
        )?["id"]
            .as_u64()
            .unwrap_or(0);
        let due = run(&mut s, "flashcard.due", json!({"now_ms": t0}))?;
        assert_eq!(due["total_due"], 1);

        let after = run(
            &mut s,
            "flashcard.review",
            json!({"id": id, "grade": "Good", "now_ms": t0}),
        )?;
        assert_eq!(after["interval_days"], 1);
        let due_now = run(&mut s, "flashcard.due", json!({"now_ms": t0}))?;
        assert_eq!(due_now["total_due"], 0);
        let due_later = run(
            &mut s,
            "flashcard.due",
            json!({"now_ms": t0 + 2 * 86_400_000}),
        )?;
        assert_eq!(due_later["total_due"], 1);

        run(&mut s, "flashcard.remove", json!({"id": id}))?;
        assert!(s.doc.flashcards.is_empty());
        run(&mut s, "edit.undo", json!({}))?;
        assert_eq!(s.doc.flashcards.len(), 1);
        Ok(())
    }

    #[test]
    fn study_prefs_update_selectively_and_reject_empty_requests() -> Result<(), EngineError> {
        let mut s = Session::default();
        assert!(run(&mut s, "prefs.setStudy", json!({})).is_err());
        assert!(run(&mut s, "prefs.setStudy", json!({"left_handed": "yes"})).is_err());
        let v = run(&mut s, "prefs.setStudy", json!({"finger_drawing": true}))?;
        assert_eq!(v["finger_drawing"], true);
        assert_eq!(v["left_handed"], false);
        run(
            &mut s,
            "prefs.setStudy",
            json!({"left_handed": true, "pdf_dark_mode": true}),
        )?;
        assert!(s.prefs.left_handed && s.prefs.finger_drawing && s.prefs.pdf_dark_mode);
        Ok(())
    }

    #[test]
    fn flashcard_commands_reject_hostile_input() {
        let mut s = Session::default();
        for bad in [
            json!({"front": "", "back": "x"}),
            json!({"front": "x", "back": "   "}),
            json!({"front": "x", "back": "y", "source_page": 99}),
            json!({"front": 1, "back": 2}),
        ] {
            assert!(run(&mut s, "flashcard.add", bad).is_err());
        }
        assert!(
            run(
                &mut s,
                "flashcard.review",
                json!({"id": 1, "grade": "Good"})
            )
            .is_err()
        );
        assert!(
            run(
                &mut s,
                "flashcard.review",
                json!({"id": 1, "grade": "Nope"})
            )
            .is_err()
        );
        assert!(s.doc.flashcards.is_empty());
    }
}
