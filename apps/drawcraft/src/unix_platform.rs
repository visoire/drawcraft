//! Safe Linux and macOS platform hooks for native file dialogs, clipboard text/image
//! transfer, external URLs, and connected pen digitizer detection.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use drawcraft_tablet::ClipboardImage;
use drawcraft_ui::{ClipboardImagePayload, FileDialogKind, PickedOpenFile, PlatformHooks};

use crate::{cached_dir, is_safe_external_url, remember_parent_dir};

const MAX_CLIPBOARD_IMAGE_BYTES: usize = 32 * 1024 * 1024;
const MAX_CLIPBOARD_TEXT_BYTES: usize = 4 * 1024 * 1024;
#[cfg(any(not(target_os = "macos"), test))]
const PREFERRED_IMAGE_MIMES: [&str; 6] = [
    "image/png",
    "image/jpeg",
    "image/webp",
    "image/bmp",
    "image/gif",
    "image/tiff",
];

pub fn install_platform_hooks() -> (Option<()>, PlatformHooks) {
    let last_dir: Arc<Mutex<Option<PathBuf>>> = Arc::new(Mutex::new(None));
    let save_dir = Arc::clone(&last_dir);
    let open_dir = Arc::clone(&last_dir);
    let image_dir = Arc::clone(&last_dir);

    let hooks = PlatformHooks {
        #[cfg(target_os = "linux")]
        connected_pen_devices: Some(Box::new(|| {
            drawcraft_tablet::linux::connected_pen_devices()
                .into_iter()
                .map(|d| drawcraft_ui::stylus::ConnectedStylusInfo {
                    name: d.name,
                    is_external: d.kind == drawcraft_tablet::PenDeviceKind::External,
                })
                .collect()
        })),
        pick_save_path: Some(Box::new(move |kind: FileDialogKind, default_name: &str| {
            let init = cached_dir(&save_dir);
            let chosen = pick_save_path_unix(kind, default_name, init.as_deref())?;
            remember_parent_dir(&save_dir, &chosen);
            Some(chosen)
        })),
        pick_open_file: Some(Box::new(move |kind: FileDialogKind| {
            let init = cached_dir(&open_dir);
            let chosen = pick_open_path_unix(kind, init.as_deref())?;
            remember_parent_dir(&open_dir, &chosen);
            Some(PickedOpenFile::Path(chosen))
        })),
        pick_image_file: Some(Box::new(move || {
            let init = cached_dir(&image_dir);
            let chosen = pick_open_image_path_unix(init.as_deref())?;
            remember_parent_dir(&image_dir, &chosen);
            Some(PickedOpenFile::Path(chosen))
        })),
        clipboard_image: Some(Box::new(|| {
            read_clipboard_image_unix().map(to_ui_clipboard_image)
        })),
        clipboard_image_candidates: Some(Box::new(|| {
            read_clipboard_image_candidates_unix()
                .into_iter()
                .map(to_ui_clipboard_image)
                .collect()
        })),
        clipboard_text: Some(Box::new(read_clipboard_text_unix)),
        set_clipboard_text: Some(Box::new(|text| {
            let _ = write_clipboard_text_unix(text);
        })),
        open_url: Some(Box::new(|url| {
            let trimmed = url.trim();
            if is_safe_external_url(trimmed) {
                #[cfg(target_os = "macos")]
                let _ = Command::new("open").arg(trimmed).spawn();
                #[cfg(not(target_os = "macos"))]
                let _ = Command::new("xdg-open").arg(trimmed).spawn();
            }
        })),
        ..PlatformHooks::default()
    };

    (None, hooks)
}

fn to_ui_clipboard_image(img: ClipboardImage) -> ClipboardImagePayload {
    match img {
        ClipboardImage::EncodedBytes(bytes) => ClipboardImagePayload::EncodedBytes(bytes),
        ClipboardImage::FilePath(path) => ClipboardImagePayload::FilePath(path),
    }
}

fn run_stdout_bytes(program: &str, args: &[&str], max_bytes: usize) -> Option<Vec<u8>> {
    let out = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() || out.stdout.is_empty() || out.stdout.len() > max_bytes {
        return None;
    }
    Some(out.stdout)
}

fn run_unix_dialog_output(program: &str, args: &[&str]) -> Option<PathBuf> {
    let out = Command::new(program).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!text.is_empty()).then(|| PathBuf::from(text))
}

fn pipe_to_command(program: &str, args: &[&str], text: &str) -> bool {
    let Ok(mut child) = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(text.as_bytes());
    }
    child.wait().is_ok_and(|s| s.success())
}

fn pick_save_path_unix(
    kind: FileDialogKind,
    default_name: &str,
    initial_dir: Option<&Path>,
) -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let escaped_name = default_name.replace('"', "");
        let script = match initial_dir {
            Some(dir) => format!(
                "POSIX path of (choose file name with prompt \"Save {}\" default location POSIX file \"{}\" default name \"{}\")",
                kind.filter_label(),
                dir.display(),
                escaped_name
            ),
            None => format!(
                "POSIX path of (choose file name with prompt \"Save {}\" default name \"{}\")",
                kind.filter_label(),
                escaped_name
            ),
        };
        run_unix_dialog_output("osascript", &["-e", &script])
    }
    #[cfg(not(target_os = "macos"))]
    {
        let start = initial_dir
            .unwrap_or_else(|| Path::new("."))
            .join(default_name)
            .to_string_lossy()
            .into_owned();
        let filter = format!("--file-filter=*.{}", kind.extension());
        run_unix_dialog_output(
            "zenity",
            &[
                "--file-selection",
                "--save",
                "--confirm-overwrite",
                "--filename",
                &start,
                &filter,
            ],
        )
        .or_else(|| {
            let kfilter = format!("*.{}|{}", kind.extension(), kind.filter_label());
            run_unix_dialog_output("kdialog", &["--getsavefilename", &start, &kfilter])
        })
    }
}

fn pick_open_path_unix(kind: FileDialogKind, initial_dir: Option<&Path>) -> Option<PathBuf> {
    let exts = kind.open_extensions();
    #[cfg(target_os = "macos")]
    {
        let _ = initial_dir;
        let type_list = exts
            .iter()
            .map(|ext| format!("\"{ext}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let script = format!(
            "POSIX path of (choose file with prompt \"Open {}\" of type {{{type_list}}})",
            kind.filter_label(),
        );
        run_unix_dialog_output("osascript", &["-e", &script])
    }
    #[cfg(not(target_os = "macos"))]
    {
        let dir_str = initial_dir
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let glob_list = exts
            .iter()
            .map(|ext| format!("*.{ext}"))
            .collect::<Vec<_>>()
            .join(" ");
        let filter = format!("--file-filter={} | {glob_list}", kind.filter_label());
        run_unix_dialog_output(
            "zenity",
            &["--file-selection", "--filename", &dir_str, &filter],
        )
        .or_else(|| {
            let kfilter = format!("{glob_list}|{}", kind.filter_label());
            run_unix_dialog_output("kdialog", &["--getopenfilename", &dir_str, &kfilter])
        })
    }
}

fn pick_open_image_path_unix(initial_dir: Option<&Path>) -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let _ = initial_dir;
        let script =
            "POSIX path of (choose file with prompt \"Insert Image\" of type {\"public.image\"})";
        run_unix_dialog_output("osascript", &["-e", script])
    }
    #[cfg(not(target_os = "macos"))]
    {
        let dir_str = initial_dir
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let filter = "--file-filter=Image Files | *.png *.jpg *.jpeg *.jfif *.webp *.gif *.bmp *.ico *.tif *.tiff *.tga *.qoi";
        run_unix_dialog_output(
            "zenity",
            &["--file-selection", "--filename", &dir_str, filter],
        )
        .or_else(|| {
            let kfilter =
                "*.png *.jpg *.jpeg *.jfif *.webp *.gif *.bmp *.ico *.tif *.tiff|Image Files";
            run_unix_dialog_output("kdialog", &["--getopenfilename", &dir_str, kfilter])
        })
    }
}

fn read_clipboard_text_unix() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let bytes = run_stdout_bytes("pbpaste", &[], MAX_CLIPBOARD_TEXT_BYTES)?;
        String::from_utf8(bytes).ok()
    }
    #[cfg(not(target_os = "macos"))]
    {
        let bytes = run_stdout_bytes(
            "wl-paste",
            &["--no-newline", "--type", "text/plain;charset=utf-8"],
            MAX_CLIPBOARD_TEXT_BYTES,
        )
        .or_else(|| run_stdout_bytes("wl-paste", &["--no-newline"], MAX_CLIPBOARD_TEXT_BYTES))
        .or_else(|| {
            run_stdout_bytes(
                "xclip",
                &["-selection", "clipboard", "-o"],
                MAX_CLIPBOARD_TEXT_BYTES,
            )
        })
        .or_else(|| {
            run_stdout_bytes(
                "xsel",
                &["--clipboard", "--output"],
                MAX_CLIPBOARD_TEXT_BYTES,
            )
        })?;
        String::from_utf8(bytes).ok()
    }
}

fn write_clipboard_text_unix(text: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        pipe_to_command("pbcopy", &[], text)
    }
    #[cfg(not(target_os = "macos"))]
    {
        pipe_to_command("wl-copy", &[], text)
            || pipe_to_command("xclip", &["-selection", "clipboard"], text)
            || pipe_to_command("xsel", &["--clipboard", "--input"], text)
    }
}

fn read_clipboard_image_unix() -> Option<ClipboardImage> {
    read_clipboard_image_candidates_unix().into_iter().next()
}

fn read_clipboard_image_candidates_unix() -> Vec<ClipboardImage> {
    #[cfg(target_os = "macos")]
    {
        read_macos_clipboard_images()
    }
    #[cfg(not(target_os = "macos"))]
    {
        read_linux_clipboard_images()
    }
}

#[cfg(target_os = "macos")]
fn read_macos_clipboard_images() -> Vec<ClipboardImage> {
    let mut out = Vec::new();
    for class_name in ["PNGf", "TIFF"] {
        let script = format!("try\nreturn the clipboard as «class {class_name}»\nend try");
        if let Some(raw) =
            run_stdout_bytes("osascript", &["-e", &script], MAX_CLIPBOARD_IMAGE_BYTES * 2)
            && let Ok(text) = std::str::from_utf8(&raw)
            && let Some(bytes) = parse_osascript_hex_data(text, MAX_CLIPBOARD_IMAGE_BYTES)
        {
            out.push(ClipboardImage::EncodedBytes(bytes));
        }
    }
    out
}

#[cfg(not(target_os = "macos"))]
fn read_linux_clipboard_images() -> Vec<ClipboardImage> {
    if let Some(types_raw) = run_stdout_bytes("wl-paste", &["--list-types"], 64 * 1024)
        && let Ok(types_str) = std::str::from_utf8(&types_raw)
    {
        let items = collect_linux_clipboard_from_types(types_str, |mime| {
            run_stdout_bytes("wl-paste", &["--type", mime], MAX_CLIPBOARD_IMAGE_BYTES)
        });
        if !items.is_empty() {
            return items;
        }
    }

    if let Some(types_raw) = run_stdout_bytes(
        "xclip",
        &["-selection", "clipboard", "-t", "TARGETS", "-o"],
        64 * 1024,
    ) && let Ok(types_str) = std::str::from_utf8(&types_raw)
    {
        return collect_linux_clipboard_from_types(types_str, |mime| {
            run_stdout_bytes(
                "xclip",
                &["-selection", "clipboard", "-t", mime, "-o"],
                MAX_CLIPBOARD_IMAGE_BYTES,
            )
        });
    }

    Vec::new()
}

#[cfg(not(target_os = "macos"))]
fn collect_linux_clipboard_from_types(
    types_listing: &str,
    mut fetch_mime: impl FnMut(&str) -> Option<Vec<u8>>,
) -> Vec<ClipboardImage> {
    let mut results = Vec::new();
    if let Some(mime) = pick_best_image_mime(types_listing)
        && let Some(bytes) = fetch_mime(mime)
    {
        results.push(ClipboardImage::EncodedBytes(bytes));
    }
    if types_listing
        .lines()
        .any(|line| line.trim().eq_ignore_ascii_case("text/uri-list"))
        && let Some(raw_uris) = fetch_mime("text/uri-list")
        && let Ok(uri_text) = std::str::from_utf8(&raw_uris)
    {
        for path in parse_file_uri_list(uri_text) {
            results.push(ClipboardImage::FilePath(path));
        }
    }
    results
}

#[cfg(any(not(target_os = "macos"), test))]
fn pick_best_image_mime(types_listing: &str) -> Option<&'static str> {
    PREFERRED_IMAGE_MIMES.into_iter().find(|preferred| {
        types_listing
            .lines()
            .any(|line| line.trim().eq_ignore_ascii_case(preferred))
    })
}

#[cfg(any(not(target_os = "macos"), test))]
fn parse_file_uri_list(uri_list: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for line in uri_list.lines().take(32) {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some(rest) = trimmed
            .strip_prefix("file://localhost")
            .or_else(|| trimmed.strip_prefix("file://"))
        else {
            continue;
        };
        if !rest.starts_with('/') {
            continue;
        }
        if let Some(decoded) = percent_decode_utf8(rest) {
            paths.push(PathBuf::from(decoded));
        }
    }
    paths
}

#[cfg(any(not(target_os = "macos"), test))]
fn percent_decode_utf8(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = hex_nibble(bytes[i + 1])?;
            let lo = hex_nibble(bytes[i + 2])?;
            out.push((hi << 4) | lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(any(target_os = "macos", test))]
fn parse_osascript_hex_data(output: &str, max_bytes: usize) -> Option<Vec<u8>> {
    let trimmed = output.trim();
    let inner = trimmed
        .strip_prefix("«data ")
        .and_then(|s| s.strip_suffix('»'))?;
    if inner.len() < 4 {
        return None;
    }
    let hex = inner.get(4..)?.trim();
    if hex.is_empty() || !hex.len().is_multiple_of(2) || hex.len() / 2 > max_bytes {
        return None;
    }
    let raw = hex.as_bytes();
    let mut bytes = Vec::with_capacity(raw.len() / 2);
    let mut i = 0;
    while i + 1 < raw.len() {
        let hi = hex_nibble(raw[i])?;
        let lo = hex_nibble(raw[i + 1])?;
        bytes.push((hi << 4) | lo);
        i += 2;
    }
    Some(bytes)
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn picks_preferred_image_mime_in_priority_order() {
        assert_eq!(
            pick_best_image_mime("text/plain\nimage/jpeg\nimage/png\n"),
            Some("image/png")
        );
        assert_eq!(
            pick_best_image_mime("text/html\nimage/webp\n"),
            Some("image/webp")
        );
        assert_eq!(pick_best_image_mime("text/plain\nUTF8_STRING\n"), None);
    }

    #[test]
    fn parses_file_uri_list_with_percent_encoding() {
        let uris = "# comment\nfile:///home/user/My%20Notes/sketch%201.png\nhttps://example.com/a.png\nfile://localhost/tmp/photo.jpg\n";
        let paths = parse_file_uri_list(uris);
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/home/user/My Notes/sketch 1.png"),
                PathBuf::from("/tmp/photo.jpg"),
            ]
        );
    }

    #[test]
    fn parses_osascript_png_hex_and_rejects_malformed() {
        assert_eq!(
            parse_osascript_hex_data("«data PNGf89504E47»\n", 1024),
            Some(vec![0x89, 0x50, 0x4E, 0x47])
        );
        assert_eq!(parse_osascript_hex_data("«data PNGf895»", 1024), None);
        assert_eq!(parse_osascript_hex_data("«data PNGfZZZZ»", 1024), None);
        assert_eq!(parse_osascript_hex_data("«data PNGf89504E47»", 2), None);
    }
}
