//! `cargo xtask layers` - enforce the `L0-L6` crate dependency boundaries.

use serde_json::Value;
use std::path::Path;
use std::process::Command;

/// Layer assigned to every workspace crate. Dependencies may only point to a lower layer,
/// or to a same-layer crate listed in [`SAME_LAYER_EDGES`].
const LAYERS: &[(&str, u8)] = &[
    ("drawcraft-geom", 0),
    ("drawcraft-color", 0),
    ("drawcraft-raster", 0),
    ("drawcraft-tablet", 0),
    ("drawcraft-doc", 1),
    ("drawcraft-ops", 2),
    ("drawcraft-paint", 2),
    ("drawcraft-sync-protocol", 2),
    ("drawcraft-format", 3),
    ("drawcraft-compose", 3),
    ("drawcraft-storage", 4),
    ("drawcraft-sync", 4),
    ("drawcraft-engine", 5),
    ("drawcraft-ui", 6),
    ("drawcraft-ffi", 6),
    ("drawcraft-web", 6),
    ("drawcraft", 6),
    ("drawcraft-cli", 6),
];

/// Explicitly permitted `(dependent, dependency)` pairs inside one layer.
const SAME_LAYER_EDGES: &[(&str, &str)] = &[
    ("drawcraft-color", "drawcraft-geom"),
    ("drawcraft-raster", "drawcraft-geom"),
    ("drawcraft-raster", "drawcraft-color"),
    ("drawcraft-web", "drawcraft-ui"),
    ("drawcraft", "drawcraft-ui"),
];

/// Third-party crates that must not appear below `L6` (no UI, windowing or browser glue in the core).
const BANNED_BELOW_L6: &[&str] = &[
    "egui",
    "eframe",
    "egui-wgpu",
    "egui_extras",
    "wgpu",
    "winit",
    "web-sys",
    "js-sys",
    "wasm-bindgen",
];

/// A workspace package and its direct dependencies.
#[derive(Debug, Clone)]
pub struct Package {
    pub name: String,
    pub deps: Vec<Dep>,
}

/// A direct dependency edge.
#[derive(Debug, Clone)]
pub struct Dep {
    pub name: String,
    /// `true` when the dependency is a local path dependency (a workspace crate).
    pub is_path: bool,
}

/// Layer number of a workspace crate, if it is known.
pub fn layer_of(name: &str) -> Option<u8> {
    LAYERS.iter().find(|(n, _)| *n == name).map(|(_, l)| *l)
}

/// Check the dependency graph and return one human-readable message per violation.
pub fn check(packages: &[Package]) -> Vec<String> {
    let mut violations = Vec::new();
    for pkg in packages {
        if pkg.name == "xtask" {
            if pkg.deps.iter().any(|d| d.is_path) {
                violations.push("xtask must not depend on workspace crates".to_string());
            }
            continue;
        }
        let Some(pkg_layer) = layer_of(&pkg.name) else {
            violations.push(format!(
                "{}: crate has no layer assignment (add it to LAYERS in xtask/src/layers.rs)",
                pkg.name
            ));
            continue;
        };
        for dep in &pkg.deps {
            if dep.is_path {
                check_internal(&pkg.name, pkg_layer, &dep.name, &mut violations);
            } else if pkg_layer < 6 && BANNED_BELOW_L6.contains(&dep.name.as_str()) {
                violations.push(format!(
                    "{} (L{pkg_layer}): UI/browser dependency `{}` is only allowed in L6",
                    pkg.name, dep.name
                ));
            }
        }
    }
    violations
}

fn check_internal(pkg: &str, pkg_layer: u8, dep: &str, violations: &mut Vec<String>) {
    if pkg == "drawcraft-tablet" {
        violations.push(format!(
            "drawcraft-tablet must have zero workspace dependencies (found `{dep}`)"
        ));
        return;
    }
    let Some(dep_layer) = layer_of(dep) else {
        violations.push(format!("{pkg}: depends on unknown workspace crate `{dep}`"));
        return;
    };
    if dep_layer > pkg_layer {
        violations.push(format!(
            "{pkg} (L{pkg_layer}) depends upward on {dep} (L{dep_layer})"
        ));
    } else if dep_layer == pkg_layer && !SAME_LAYER_EDGES.contains(&(pkg, dep)) {
        violations.push(format!(
            "{pkg} (L{pkg_layer}) depends sideways on {dep} (same layer, edge not allowed)"
        ));
    }
}

/// Parse the output of `cargo metadata --no-deps` into [`Package`]s.
pub fn parse_metadata(meta: &Value) -> Result<Vec<Package>, String> {
    let packages = meta
        .get("packages")
        .and_then(Value::as_array)
        .ok_or("cargo metadata: missing `packages` array")?;
    let mut out = Vec::with_capacity(packages.len());
    for p in packages {
        let name = p
            .get("name")
            .and_then(Value::as_str)
            .ok_or("cargo metadata: package without name")?
            .to_string();
        let mut deps = Vec::new();
        if let Some(list) = p.get("dependencies").and_then(Value::as_array) {
            for d in list {
                let dep_name = d
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or("cargo metadata: dependency without name")?;
                let is_path = d.get("path").is_some_and(|v| !v.is_null());
                deps.push(Dep {
                    name: dep_name.to_string(),
                    is_path,
                });
            }
        }
        out.push(Package { name, deps });
    }
    Ok(out)
}

/// Run the layer check for the workspace at `root`.
pub fn run(root: &Path) -> Result<(), String> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("failed to run cargo metadata: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let meta: Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("cargo metadata returned invalid JSON: {e}"))?;
    let packages = parse_metadata(&meta)?;
    let violations = check(&packages);
    if violations.is_empty() {
        println!("layers: {} crates, no boundary violations", packages.len());
        Ok(())
    } else {
        for v in &violations {
            eprintln!("layers: {v}");
        }
        Err(format!("{} layer violation(s)", violations.len()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkg(name: &str, deps: &[(&str, bool)]) -> Package {
        Package {
            name: name.to_string(),
            deps: deps
                .iter()
                .map(|(n, p)| Dep {
                    name: (*n).to_string(),
                    is_path: *p,
                })
                .collect(),
        }
    }

    #[test]
    fn downward_and_allowed_sideways_edges_pass() {
        let pkgs = [
            pkg(
                "drawcraft-raster",
                &[("drawcraft-color", true), ("drawcraft-geom", true)],
            ),
            pkg(
                "drawcraft-engine",
                &[("drawcraft-doc", true), ("drawcraft-storage", true)],
            ),
            pkg(
                "drawcraft-web",
                &[("drawcraft-ui", true), ("drawcraft-engine", true)],
            ),
        ];
        assert!(check(&pkgs).is_empty());
    }

    #[test]
    fn upward_dependency_is_rejected() {
        let pkgs = [pkg("drawcraft-doc", &[("drawcraft-engine", true)])];
        let v = check(&pkgs);
        assert_eq!(v.len(), 1);
        assert!(v[0].contains("upward"));
    }

    #[test]
    fn undeclared_sideways_dependency_is_rejected() {
        let pkgs = [pkg("drawcraft-ops", &[("drawcraft-paint", true)])];
        assert!(check(&pkgs)[0].contains("sideways"));
    }

    #[test]
    fn tablet_must_stay_isolated() {
        let pkgs = [pkg("drawcraft-tablet", &[("drawcraft-geom", true)])];
        assert!(check(&pkgs)[0].contains("zero workspace dependencies"));
    }

    #[test]
    fn ui_crates_are_banned_below_l6_but_allowed_in_l6() {
        let below = [pkg("drawcraft-compose", &[("egui", false)])];
        assert_eq!(check(&below).len(), 1);
        let at_l6 = [pkg("drawcraft-ui", &[("egui", false)])];
        assert!(check(&at_l6).is_empty());
    }

    #[test]
    fn unknown_crates_are_flagged() {
        assert_eq!(check(&[pkg("drawcraft-mystery", &[])]).len(), 1);
        let pkgs = [pkg("drawcraft-doc", &[("drawcraft-mystery", true)])];
        assert_eq!(check(&pkgs).len(), 1);
    }

    #[test]
    fn metadata_parsing_rejects_malformed_input() {
        assert!(parse_metadata(&serde_json::json!({})).is_err());
        assert!(parse_metadata(&serde_json::json!({"packages": [{}]})).is_err());
        let ok = serde_json::json!({"packages": [
            {"name": "a", "dependencies": [{"name": "b", "path": "/x"}, {"name": "c", "path": null}]}
        ]});
        let parsed = parse_metadata(&ok).unwrap_or_default();
        assert_eq!(parsed.len(), 1);
        assert!(parsed[0].deps[0].is_path);
        assert!(!parsed[0].deps[1].is_path);
    }
}
