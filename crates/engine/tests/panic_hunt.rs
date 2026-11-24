//! Adversarial fuzzing of every registered engine command ("Never crash" Golden Rule).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use drawcraft_engine::{Session, commands};
use serde_json::{Value, json};

#[test]
fn panic_hunt_all_commands_resist_adversarial_inputs() {
    let payloads: Vec<Value> = vec![
        Value::Null,
        json!(true),
        json!(false),
        json!(0),
        json!(-999_999),
        json!(1e308),
        json!("bad-string"),
        json!([]),
        json!({}),
        json!({ "points": [] }),
        json!({ "path": [] }),
        json!({ "points": [{ "x": 1e308, "y": -1e308, "pressure": -999.0, "tilt_x": 9999.0, "rotation": -5000.0 }] }),
        json!({ "path": [{ "x": 0.0, "y": 0.0, "radius": -5.0 }] }),
        json!({ "path": [{ "x": 0.0, "y": 0.0, "radius": 0.0 }] }),
        json!({ "tool": "LaserPointer" }),
        json!({ "size": -100.0, "color_hex": "not-a-hex" }),
        json!({ "points": [[0.0, 0.0]] }),
        json!({ "preset": "NonExistent" }),
        json!({ "template": "Hexagon" }),
        json!({ "target": "ink", "index": 999999, "color_hex": "#112233" }),
        json!({ "target": "highlighter", "index": 0, "color": { "r": 1e38, "g": -99.0, "b": 0.5, "a": 1.0 } }),
        json!({ "target": "unknown", "index": 0 }),
        json!({ "id": "", "name": "", "settings": {} }),
        json!({ "id": "nonexistent_preset_id" }),
        json!({ "id": "builtin_classic" }),
        json!({ "id": "builtin_pastel", "name": "" }),
        json!({ "name": " \x00 ", "ink_colors": [], "highlighter_colors": [{ "r": 1e38, "g": -9.0, "b": 0.0, "a": 1.0 }] }),
        json!({ "theme": "Dark" }),
        json!({ "theme": "NeonUltraViolet" }),
        json!({ "bytes": [1, 2, 3, 4], "x": 1e38, "y": -1e38 }),
        json!({ "base64_bytes": "not!valid!base64" }),
        json!({ "targets": [{ "kind": "image", "index": 999999 }] }),
        json!({
            "targets": [{ "kind": "shape", "index": 0 }],
            "pivot": { "x": 100.0, "y": 100.0 },
            "translation": { "x": 1e308, "y": -1e308 },
            "scale_x": 1e308,
            "scale_y": -1.0,
            "rotation_degrees": 0.0,
        }),
        json!({ "index": 0, "x": 1e308, "y": -1e308, "max_width": 1e38, "clear_max_width": true, "record_history": false }),
        json!({ "reset": true }),
        json!({ "barrel_button": "Undo", "secondary_barrel_button": "PanCanvasHold", "eraser_end": "EraserHold" }),
        json!({ "eraser_end": "Undo" }),
    ];

    for spec in commands::specs() {
        for payload in &payloads {
            let mut session = Session::default();
            // Must return Ok or Err cleanly, never panic.
            let _ = session.execute(spec.id, payload);
        }
    }

    let mut session = Session::default();
    assert!(session.execute("unknown.command", &json!({})).is_err());
}
