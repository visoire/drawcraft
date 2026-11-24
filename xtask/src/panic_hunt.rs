//! `cargo xtask panic-hunt` - zero `unwrap`/`expect`/`panic!`/`todo!`/`unimplemented!`/`unreachable!`
//! in non-test code, and every crate root must carry the required `#![deny(clippy::...)]` block.

use std::fs;
use std::path::{Path, PathBuf};

const REQUIRED_LINTS: &[&str] = &[
    "clippy::unwrap_used",
    "clippy::expect_used",
    "clippy::panic",
    "clippy::unimplemented",
    "clippy::todo",
    "clippy::unreachable",
];

const PANIC_MACROS: &[&str] = &["panic", "todo", "unimplemented", "unreachable"];
const PANIC_METHODS: &[&str] = &["unwrap", "expect"];
const TEST_ATTRS: &[&str] = &["#[cfg(test)]", "#[test]"];
const SKIP_DIRS: &[&str] = &["target", "tests", "benches", "examples", "dist", ".git"];

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn blank(out: &mut [char], from: usize, to: usize) {
    for ch in out.iter_mut().take(to).skip(from) {
        if *ch != '\n' {
            *ch = ' ';
        }
    }
}

/// Replace comments, string literals and char literals with spaces (newlines are kept so
/// line numbers stay valid).
pub fn sanitize(src: &str) -> String {
    let chars: Vec<char> = src.chars().collect();
    let mut out = chars.clone();
    let n = chars.len();
    let mut i = 0;
    while i < n {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if c == '/' && next == Some('/') {
            let start = i;
            while i < n && chars[i] != '\n' {
                i += 1;
            }
            blank(&mut out, start, i);
        } else if c == '/' && next == Some('*') {
            let start = i;
            let mut depth = 0usize;
            while i < n {
                if chars[i] == '/' && chars.get(i + 1) == Some(&'*') {
                    depth += 1;
                    i += 2;
                } else if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
                    depth = depth.saturating_sub(1);
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
            blank(&mut out, start, i);
        } else if c == 'r'
            && (i == 0 || !is_ident(chars[i - 1]))
            && raw_string_end(&chars, i).is_some()
        {
            let end = raw_string_end(&chars, i).unwrap_or(n);
            blank(&mut out, i, end);
            i = end;
        } else if c == '"' {
            let start = i;
            i += 1;
            while i < n && chars[i] != '"' {
                i += if chars[i] == '\\' { 2 } else { 1 };
            }
            i = (i + 1).min(n);
            blank(&mut out, start, i);
        } else if c == '\'' {
            // Char literal ('a', '\n') versus lifetime ('a).
            if next == Some('\\') {
                let start = i;
                i += 2;
                while i < n && chars[i] != '\'' {
                    i += 1;
                }
                i = (i + 1).min(n);
                blank(&mut out, start, i);
            } else if chars.get(i + 2) == Some(&'\'') {
                blank(&mut out, i, i + 3);
                i += 3;
            } else {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    out.into_iter().collect()
}

/// If `chars[start..]` begins a raw string literal (`r"..."`, `r#"..."#`), return the end index.
fn raw_string_end(chars: &[char], start: usize) -> Option<usize> {
    let mut j = start + 1;
    let mut hashes = 0usize;
    while chars.get(j) == Some(&'#') {
        hashes += 1;
        j += 1;
    }
    if chars.get(j) != Some(&'"') {
        return None;
    }
    j += 1;
    while j < chars.len() {
        if chars[j] == '"' && (0..hashes).all(|k| chars.get(j + 1 + k) == Some(&'#')) {
            return Some(j + 1 + hashes);
        }
        j += 1;
    }
    Some(chars.len())
}

/// Blank out items annotated with `#[cfg(test)]` / `#[test]` (to the matching `}` or `;`).
/// Expects already-sanitized text.
pub fn strip_test_items(clean: &str) -> String {
    let chars: Vec<char> = clean.chars().collect();
    let mut out = chars.clone();
    for attr in TEST_ATTRS {
        let needle: Vec<char> = attr.chars().collect();
        let mut i = 0;
        while i + needle.len() <= chars.len() {
            if chars[i..i + needle.len()] == needle[..] {
                let end = item_end(&chars, i + needle.len());
                blank(&mut out, i, end);
                i = end.max(i + needle.len());
            } else {
                i += 1;
            }
        }
    }
    out.into_iter().collect()
}

/// End (exclusive) of the item that follows an attribute, starting the search at `from`.
fn item_end(chars: &[char], from: usize) -> usize {
    let mut i = from;
    while i < chars.len() {
        match chars[i] {
            ';' => return i + 1,
            '{' => {
                let mut depth = 0usize;
                while i < chars.len() {
                    match chars[i] {
                        '{' => depth += 1,
                        '}' => {
                            depth = depth.saturating_sub(1);
                            if depth == 0 {
                                return i + 1;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                return chars.len();
            }
            _ => i += 1,
        }
    }
    chars.len()
}

/// A forbidden construct found in non-test code.
#[derive(Debug, PartialEq, Eq)]
pub struct Finding {
    pub line: usize,
    pub what: String,
}

/// Scan sanitized, test-stripped text for panicking constructs.
pub fn scan(clean: &str) -> Vec<Finding> {
    let chars: Vec<char> = clean.chars().collect();
    let mut findings = Vec::new();
    let line_of = |idx: usize| 1 + chars[..idx].iter().filter(|c| **c == '\n').count();

    let mut i = 0;
    while i < chars.len() {
        if !is_ident(chars[i]) || (i > 0 && is_ident(chars[i - 1])) {
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && is_ident(chars[i]) {
            i += 1;
        }
        let word: String = chars[start..i].iter().collect();
        let after = chars[i..].iter().find(|c| !c.is_whitespace()).copied();
        let after_idx = chars[i..]
            .iter()
            .position(|c| !c.is_whitespace())
            .map(|p| i + p);
        let preceded_by_dot =
            chars[..start].iter().rev().find(|c| !c.is_whitespace()) == Some(&'.');

        if PANIC_METHODS.contains(&word.as_str()) && preceded_by_dot && after == Some('(') {
            findings.push(Finding {
                line: line_of(start),
                what: format!(".{word}()"),
            });
        } else if PANIC_MACROS.contains(&word.as_str())
            && after == Some('!')
            && after_idx.and_then(|p| chars.get(p + 1)) != Some(&'=')
        {
            findings.push(Finding {
                line: line_of(start),
                what: format!("{word}!"),
            });
        }
    }
    findings
}

/// Whether `raw` contains an `#![deny(...)]` block naming every required lint.
pub fn has_required_deny(raw: &str) -> bool {
    let sanitized = sanitize(raw);
    let mut rest = sanitized.as_str();
    while let Some(pos) = rest.find("#![deny(") {
        let block = &rest[pos..];
        let end = block.find(")]").unwrap_or(block.len());
        let block = &block[..end];
        if REQUIRED_LINTS.iter().all(|lint| block.contains(lint)) {
            return true;
        }
        rest = &rest[pos + 1..];
    }
    false
}

fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for entry in entries {
        let path = entry.map_err(|e| format!("{}: {e}", dir.display()))?.path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if path.is_dir() {
            if !SKIP_DIRS.contains(&name) {
                collect_rs(&path, out)?;
            }
        } else if path.extension().is_some_and(|e| e == "rs") && name != "tests.rs" {
            out.push(path);
        }
    }
    Ok(())
}

fn crate_dirs(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut dirs = vec![root.join("xtask")];
    for group in ["crates", "apps"] {
        let group_dir = root.join(group);
        if !group_dir.is_dir() {
            continue;
        }
        let entries =
            fs::read_dir(&group_dir).map_err(|e| format!("{}: {e}", group_dir.display()))?;
        for entry in entries {
            let path = entry.map_err(|e| e.to_string())?.path();
            if path.join("Cargo.toml").is_file() {
                dirs.push(path);
            }
        }
    }
    dirs.sort();
    Ok(dirs)
}

/// Run the panic hunt over the workspace at `root`.
pub fn run(root: &Path) -> Result<(), String> {
    let mut problems = Vec::new();
    let mut files_scanned = 0usize;
    for dir in crate_dirs(root)? {
        for entry in ["src/lib.rs", "src/main.rs"] {
            let path = dir.join(entry);
            if path.is_file() {
                let raw =
                    fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                if !has_required_deny(&raw) {
                    problems.push(format!(
                        "{}: crate root lacks #![deny(clippy::unwrap_used, ...)] with all six lints",
                        path.display()
                    ));
                }
            }
        }
        let mut files = Vec::new();
        let src = dir.join("src");
        if src.is_dir() {
            collect_rs(&src, &mut files)?;
        }
        for file in files {
            files_scanned += 1;
            let raw = fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
            let clean = strip_test_items(&sanitize(&raw));
            for f in scan(&clean) {
                problems.push(format!(
                    "{}:{}: forbidden `{}` in non-test code",
                    file.display(),
                    f.line,
                    f.what
                ));
            }
        }
    }
    if problems.is_empty() {
        println!("panic-hunt: {files_scanned} files clean");
        Ok(())
    } else {
        for p in &problems {
            eprintln!("panic-hunt: {p}");
        }
        Err(format!("{} panic-hunt problem(s)", problems.len()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hunt(src: &str) -> Vec<Finding> {
        scan(&strip_test_items(&sanitize(src)))
    }

    #[test]
    fn flags_each_forbidden_construct_with_line_numbers() {
        let src = "fn a() {\n    x.unwrap();\n    y.expect(\"m\");\n    panic!(\"p\");\n    todo!();\n    unimplemented!();\n    unreachable!();\n}\n";
        let lines: Vec<usize> = hunt(src).iter().map(|f| f.line).collect();
        assert_eq!(lines, vec![2, 3, 4, 5, 6, 7]);
    }

    #[test]
    fn ignores_safe_lookalikes() {
        let src = "fn a() { x.unwrap_or(1); x.unwrap_or_else(f); x.expect_err(e); let panic = 1; if panic != 2 {} }";
        assert!(hunt(src).is_empty());
    }

    #[test]
    fn ignores_comments_strings_and_raw_strings() {
        let src = "// x.unwrap()\n/* panic!() /* nested todo!() */ */\nfn a() { let _ = \"x.unwrap()\"; let _ = r#\"panic!(\"hi\")\"#; let _ = '\"'; }";
        assert!(hunt(src).is_empty());
    }

    #[test]
    fn lifetimes_do_not_swallow_following_code() {
        let src = "fn a<'a>(x: &'a Option<u8>) -> u8 { x.unwrap() }";
        assert_eq!(hunt(src).len(), 1);
    }

    #[test]
    fn test_modules_and_test_fns_are_exempt() {
        let src = "fn ok() {}\n#[cfg(test)]\nmod tests {\n    fn t() { Some(1).unwrap(); }\n}\n#[test]\nfn lone() { panic!(); }\nfn bad() { None::<u8>.unwrap(); }\n";
        let found = hunt(src);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].line, 8);
    }

    #[test]
    fn crate_root_deny_block_is_validated() {
        let good = "#![deny(\n clippy::unwrap_used,\n clippy::expect_used,\n clippy::panic,\n clippy::unimplemented,\n clippy::todo,\n clippy::unreachable\n)]";
        assert!(has_required_deny(good));
        assert!(!has_required_deny("#![deny(clippy::unwrap_used)]"));
        assert!(!has_required_deny(
            "// #![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]"
        ));
        assert!(!has_required_deny(""));
    }
}
