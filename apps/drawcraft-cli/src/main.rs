#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use drawcraft_compose::render_document;
use drawcraft_doc::StrokeSample;
use drawcraft_engine::Session;
use drawcraft_engine::commands;
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Parser)]
#[command(
    name = "drawcraft-cli",
    about = "Headless CLI for DrawCraft drawing engine verification and rendering"
)]
struct Cli {
    #[command(subcommand)]
    command: Subcommands,
}

#[derive(Subcommand)]
enum Subcommands {
    /// Print all registered DrawCraft engine commands.
    Commands,
    /// Execute a JSON array of engine commands and render the final page to PNG.
    Run {
        /// Path to a JSON script containing an array of {"id": "...", "params": {...}} objects.
        script: PathBuf,
        /// Output PNG file path.
        #[arg(short, long)]
        output: PathBuf,
    },
    /// Render a handwriting, pressure, highlighter, eraser, and stabilization showcase PNG.
    Demo {
        /// Output PNG file path.
        #[arg(short, long, default_value = "target/demo_showcase.png")]
        output: PathBuf,
    },
}

#[derive(Deserialize)]
struct ScriptStep {
    id: String,
    #[serde(default = "default_params")]
    params: Value,
}

fn default_params() -> Value {
    Value::Object(serde_json::Map::new())
}

fn write_png_bytes(output: &Path, png: &[u8]) -> Result<(), String> {
    if let Some(parent) = output.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|e| {
            format!(
                "failed to create output directory `{}`: {e}",
                parent.display()
            )
        })?;
    }
    fs::write(output, png).map_err(|e| format!("failed to write `{}`: {e}", output.display()))
}

fn render_session_png(session: &Session) -> Result<Vec<u8>, String> {
    let surface = render_document(&session.doc, None);
    surface
        .encode_png()
        .map_err(|e| format!("PNG encoding failed: {e}"))
}

fn run_script(script_path: &Path, output: &Path) -> Result<(), String> {
    let raw = fs::read_to_string(script_path)
        .map_err(|e| format!("failed to read `{}`: {e}", script_path.display()))?;
    let steps: Vec<ScriptStep> =
        serde_json::from_str(&raw).map_err(|e| format!("invalid script JSON: {e}"))?;

    let mut session = Session::default();
    for (idx, step) in steps.iter().enumerate() {
        session
            .execute(&step.id, &step.params)
            .map_err(|e| format!("step {idx} (`{}`) failed: {e}", step.id))?;
    }
    let png = render_session_png(&session)?;
    write_png_bytes(output, &png)
}

fn exec(session: &mut Session, id: &str, params: Value) -> Result<(), String> {
    session
        .execute(id, &params)
        .map(|_| ())
        .map_err(|e| format!("command `{id}` failed: {e}"))
}

fn sample_series<F>(count: usize, step_ms: f64, mut f: F) -> Vec<StrokeSample>
where
    F: FnMut(usize, f64) -> (f64, f64, f32, f32, f32),
{
    let denom = (count.saturating_sub(1)).max(1) as f64;
    (0..count)
        .map(|i| {
            let t = i as f64 / denom;
            let (x, y, pressure, tilt_x, tilt_y) = f(i, t);
            StrokeSample {
                x,
                y,
                pressure,
                tilt_x,
                tilt_y,
                rotation: 0.0,
                time_ms: (i as f64) * step_ms,
            }
        })
        .collect()
}

fn add_fountain_showcase(session: &mut Session) -> Result<(), String> {
    exec(session, "tool.select", json!({ "tool": "FountainPen" }))?;
    exec(
        session,
        "pen.setSettings",
        json!({
            "kind": "FountainPen",
            "color_hex": "#1B365D",
            "size": 12.0,
            "pressure_size_min": 0.18,
            "nib_flatness": 0.35,
            "nib_angle_deg": -35.0
        }),
    )?;

    // Cursive loops with strong downstroke/upstroke pressure modulation.
    let loops = sample_series(160, 7.0, |_i, t| {
        let theta = t * std::f64::consts::TAU * 4.5;
        let x = 90.0 + t * 980.0 - theta.cos() * 26.0;
        let y = 175.0 + theta.sin() * 58.0 + (t * 6.0).sin() * 14.0;
        let downstroke = ((-theta.cos()) * 0.5 + 0.5) as f32;
        let pressure = 0.22 + downstroke * 0.72;
        (x, y, pressure, 12.0, -8.0)
    });
    exec(session, "stroke.draw", json!({ "points": loops }))?;

    // Figure-8 calligraphic flourish below loops.
    let flourish = sample_series(110, 8.0, |_i, t| {
        let theta = t * std::f64::consts::TAU * 2.0;
        let x = 140.0 + t * 880.0 + (theta * 1.5).sin() * 18.0;
        let y = 290.0 + theta.sin() * 34.0;
        let pressure = 0.25 + ((theta.cos() * 0.5 + 0.5) as f32) * 0.65;
        (x, y, pressure, 10.0, -5.0)
    });
    exec(session, "stroke.draw", json!({ "points": flourish }))
}

fn add_ballpen_and_brush_showcase(session: &mut Session) -> Result<(), String> {
    // Crisp Ball Pen handwriting zig-zag rows on ruled lines.
    exec(session, "tool.select", json!({ "tool": "BallPen" }))?;
    exec(
        session,
        "pen.setSettings",
        json!({
            "kind": "BallPen",
            "color_hex": "#1E232B",
            "size": 4.4
        }),
    )?;
    for row in 0..2 {
        let base_y = 395.0 + (row as f64) * 56.0;
        let pts = sample_series(140, 6.0, |_i, t| {
            let wave = (t * 28.0).sin();
            let envelope = (t * 9.0).cos() * 0.4 + 0.6;
            let x = 95.0 + t * 990.0;
            let y = base_y + wave * 16.0 * envelope;
            let pressure = 0.48 + (wave.abs() as f32) * 0.30;
            (x, y, pressure, 0.0, 0.0)
        });
        exec(session, "stroke.draw", json!({ "points": pts }))?;
    }

    // Expressive Brush Pen sweep with heavy pressure peaks and stylus tilt.
    exec(session, "tool.select", json!({ "tool": "BrushPen" }))?;
    exec(
        session,
        "pen.setSettings",
        json!({
            "kind": "BrushPen",
            "color_hex": "#B8253E",
            "size": 26.0,
            "pressure_size_min": 0.08,
            "tilt_sensitivity": 0.85
        }),
    )?;
    let brush = sample_series(130, 8.0, |_i, t| {
        let theta = t * std::f64::consts::PI * 3.0;
        let x = 95.0 + t * 990.0;
        let y = 570.0 + theta.sin() * 44.0;
        let pressure = 0.12 + ((theta * 0.5).sin().abs() as f32) * 0.86;
        let tilt_x = (theta.cos() * 34.0) as f32;
        let tilt_y = (theta.sin() * 24.0) as f32;
        (x, y, pressure, tilt_x, tilt_y)
    });
    exec(session, "stroke.draw", json!({ "points": brush }))
}

fn add_highlighter_and_eraser_showcase(session: &mut Session) -> Result<(), String> {
    // Black Ball Pen line crossed directly by translucent Multiply Highlighter.
    exec(
        session,
        "pen.setSettings",
        json!({
            "kind": "BallPen",
            "color_hex": "#161920",
            "size": 5.0
        }),
    )?;
    let black_line = sample_series(100, 6.0, |_i, t| {
        let x = 110.0 + t * 960.0;
        let y = 710.0 + (t * 22.0).sin() * 14.0;
        (x, y, 0.72, 0.0, 0.0)
    });
    exec(
        session,
        "stroke.draw",
        json!({ "kind": "BallPen", "points": black_line }),
    )?;

    exec(
        session,
        "pen.setSettings",
        json!({
            "kind": "Highlighter",
            "color_hex": "#FFDC32",
            "size": 36.0,
            "opacity": 0.48
        }),
    )?;
    let highlight = sample_series(60, 8.0, |_i, t| {
        let x = 100.0 + t * 980.0;
        let y = 710.0 + (t * 3.0).sin() * 3.0;
        (x, y, 1.0, 0.0, 0.0)
    });
    exec(
        session,
        "stroke.draw",
        json!({ "kind": "Highlighter", "points": highlight }),
    )?;

    // Horizontal teal Fountain Pen line split cleanly down its middle by PrecisionEraser.
    exec(
        session,
        "pen.setSettings",
        json!({
            "kind": "FountainPen",
            "color_hex": "#0F766E",
            "size": 10.0
        }),
    )?;
    let teal_line = sample_series(90, 6.0, |_i, t| {
        let x = 110.0 + t * 960.0;
        let y = 835.0 + (t * 6.0).sin() * 6.0;
        (x, y, 0.75, 0.0, 0.0)
    });
    exec(
        session,
        "stroke.draw",
        json!({ "kind": "FountainPen", "points": teal_line }),
    )?;

    exec(
        session,
        "stroke.erase",
        json!({
            "mode": "PrecisionEraser",
            "radius": 28.0,
            "path": [
                { "x": 590.0, "y": 790.0 },
                { "x": 590.0, "y": 835.0 },
                { "x": 590.0, "y": 880.0 }
            ]
        }),
    )
}

fn add_stabilizer_comparison(session: &mut Session) -> Result<(), String> {
    let configs = [
        (
            "#64748B",
            json!({
                "kind": "BallPen",
                "stabilizer_mode": "None",
                "stabilizer_strength": 0.0,
                "catmull_rom_subdivision": false,
                "streamline_amount": 0.0
            }),
        ),
        (
            "#2563EB",
            json!({
                "kind": "BallPen",
                "stabilizer_mode": "OneEuro",
                "stabilizer_strength": 0.72,
                "catmull_rom_subdivision": true,
                "streamline_amount": 0.25
            }),
        ),
        (
            "#059669",
            json!({
                "kind": "BallPen",
                "stabilizer_mode": "PulledString",
                "stabilizer_strength": 0.65,
                "catmull_rom_subdivision": true,
                "streamline_amount": 0.20,
                "catch_up_on_end": true
            }),
        ),
        (
            "#7C3AED",
            json!({
                "kind": "BallPen",
                "stabilizer_mode": "GaussianWindow",
                "stabilizer_strength": 0.70,
                "catmull_rom_subdivision": true,
                "streamline_amount": 0.65
            }),
        ),
    ];

    exec(
        session,
        "pen.setSettings",
        json!({ "kind": "BallPen", "size": 5.0 }),
    )?;

    for (idx, (hex, smoothing_patch)) in configs.iter().enumerate() {
        exec(
            session,
            "pen.setSettings",
            json!({ "kind": "BallPen", "color_hex": hex, "size": 5.2 }),
        )?;
        exec(session, "pen.setSmoothing", smoothing_patch.clone())?;

        let base_y = 980.0 + (idx as f64) * 125.0;
        let jittered = sample_series(160, 7.0, |i, t| {
            let phase = (i as f64) * 1.37;
            let jitter_x = phase.sin() * 4.5 + (phase * 2.7).cos() * 3.0;
            let jitter_y = (phase * 1.9).cos() * 11.5 + (phase * 3.3).sin() * 6.0;
            let x = 110.0 + t * 960.0 + jitter_x;
            let y = base_y + (t * std::f64::consts::TAU * 2.0).sin() * 28.0 + jitter_y;
            (x, y, 0.72, 0.0, 0.0)
        });
        exec(
            session,
            "stroke.draw",
            json!({ "kind": "BallPen", "points": jittered }),
        )?;
    }
    Ok(())
}

fn build_demo_session() -> Result<Session, String> {
    let mut session = Session::default();
    exec(
        &mut session,
        "page.setTemplate",
        json!({ "template": "RuledNarrow", "color": "WarmCream" }),
    )?;
    add_fountain_showcase(&mut session)?;
    add_ballpen_and_brush_showcase(&mut session)?;
    add_highlighter_and_eraser_showcase(&mut session)?;
    add_stabilizer_comparison(&mut session)?;
    Ok(session)
}

fn run_demo(output: &Path) -> Result<(), String> {
    let session = build_demo_session()?;
    let png = render_session_png(&session)?;
    write_png_bytes(output, &png)?;
    println!(
        "Rendered DrawCraft demo showcase ({} strokes, {}x{} px, {} bytes) -> {}",
        session.doc.strokes.len(),
        session.doc.width,
        session.doc.height,
        png.len(),
        output.display()
    );
    Ok(())
}

fn print_commands() {
    for spec in commands::specs() {
        println!("{:<22} {:<26} {}", spec.id, spec.label, spec.description);
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Subcommands::Commands => {
            print_commands();
            Ok(())
        }
        Subcommands::Run { script, output } => run_script(&script, &output),
        Subcommands::Demo { output } => run_demo(&output),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn engine_commands_list_is_complete() {
        let specs = commands::specs();
        assert!(specs.len() >= 11);
        assert!(specs.iter().any(|c| c.id == "stroke.draw"));
        assert!(specs.iter().any(|c| c.id == "stroke.erase"));
        assert!(specs.iter().any(|c| c.id == "pen.setSmoothing"));
    }

    #[test]
    fn demo_showcase_renders_valid_non_empty_png() {
        let session = build_demo_session().unwrap();
        // Split teal line produces 2 strokes, plus loops, flourish, 2 ballpen, brush, black+highlight, 4 stabilizer rows = 13
        assert!(
            session.doc.strokes.len() >= 12,
            "expected split stroke + all showcase strokes, got {}",
            session.doc.strokes.len()
        );
        let png = render_session_png(&session).unwrap();
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert!(png.len() > 20_000, "unexpectedly small PNG: {}", png.len());
    }
}
