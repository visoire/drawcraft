//! `cargo xtask web | dev | web-build` - serve and build the Pure-Rust WebAssembly PWA.
//!
//! `dev` is the one-command iPad workflow: it serves on all interfaces, prints the LAN URL to
//! open in Safari, and explains what Safari does and does not allow over plain HTTP.

use std::net::{IpAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const PORT: u16 = 8080;

/// Files a deployable bundle must contain (names are stable because `filehash = false`).
const REQUIRED_DIST_FILES: &[&str] = &[
    "index.html",
    "manifest.webmanifest",
    "sw.js",
    "storage_adapter.js",
    "icons/icon-192.png",
    "icons/icon-512.png",
    "icons/icon-maskable-512.png",
    "icons/apple-touch-icon-180.png",
];

/// Directory of the web app crate.
pub fn app_dir(root: &Path) -> PathBuf {
    root.join("apps").join("drawcraft-web")
}

/// Primary LAN address of this machine, found by asking the OS which interface would route to a
/// public address. `connect` on a UDP socket sends no packet.
pub fn lan_ip() -> Option<IpAddr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?;
    let ip = socket.local_addr().ok()?.ip();
    (!ip.is_loopback() && !ip.is_unspecified()).then_some(ip)
}

/// Whether `ip` is a private (LAN) address; a tunnel or public address is not what a tablet on
/// the same Wi-Fi can reach.
pub fn is_private_lan(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_private() || v4.is_link_local(),
        IpAddr::V6(v6) => (v6.segments()[0] & 0xfe00) == 0xfc00,
    }
}

/// Human-readable instructions printed before the dev server starts.
pub fn dev_banner(ip: Option<IpAddr>, port: u16, https: bool) -> String {
    let scheme = if https { "https" } else { "http" };
    let mut out = String::new();
    out.push_str("DrawCraft dev server\n");
    out.push_str(&format!("  this computer : {scheme}://localhost:{port}\n"));
    match ip {
        Some(ip) if is_private_lan(ip) => {
            out.push_str(&format!("  iPad / tablet : {scheme}://{ip}:{port}   (same Wi-Fi network)\n"));
        }
        Some(ip) => out.push_str(&format!(
            "  iPad / tablet : {scheme}://{ip}:{port}   (this address is not a private LAN address; check your network)\n"
        )),
        None => out.push_str("  iPad / tablet : no network address found; connect to Wi-Fi and retry\n"),
    }
    if https {
        out.push_str("\nHTTPS is on: the service worker, OPFS and 'Add to Home Screen' work on the iPad\n(the certificate must be trusted by the iPad, e.g. created with mkcert).\n");
    } else {
        out.push_str(
            "\nPlain HTTP on a LAN address is not a secure context. Safari then disables the service\n\
             worker and OPFS: the app runs and saves to IndexedDB, but offline install is unavailable.\n\
             For the full PWA test run `cargo xtask dev --https` (see apps/drawcraft-web/README.md).\n",
        );
    }
    out.push_str("\nPress Ctrl+C to stop. The page reloads when you change Rust code.\n");
    out
}

/// Paths of the TLS files used by `dev --https`.
pub fn tls_paths(app: &Path) -> (PathBuf, PathBuf) {
    let dir = app.join("dev-cert");
    (dir.join("cert.pem"), dir.join("key.pem"))
}

fn ensure_trunk() -> Result<(), String> {
    if Command::new("trunk").arg("--version").output().is_err() {
        return Err(
            "`trunk` was not found on PATH. Install it once with `cargo install trunk --locked`, \
             then rerun this command. Nothing was started."
                .to_string(),
        );
    }
    Ok(())
}

fn ensure_wasm_target() -> Result<(), String> {
    let output = Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output()
        .map_err(|e| format!("could not run rustup: {e}"))?;
    if String::from_utf8_lossy(&output.stdout).contains("wasm32-unknown-unknown") {
        Ok(())
    } else {
        Err("the wasm32-unknown-unknown target is missing: run `rustup target add wasm32-unknown-unknown`".to_string())
    }
}

fn trunk_in(app: &Path, args: &[&str]) -> Result<(), String> {
    println!("==> trunk {}", args.join(" "));
    let status = Command::new("trunk")
        .args(args)
        .current_dir(app)
        .status()
        .map_err(|e| format!("failed to start trunk: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("trunk {} exited with {status}", args.join(" ")))
    }
}

/// `cargo xtask web`: serve on localhost only.
pub fn serve_local(root: &Path) -> Result<(), String> {
    ensure_trunk()?;
    ensure_wasm_target()?;
    println!(
        "{}",
        dev_banner(None, PORT, false).replace(
            "no network address found; connect to Wi-Fi and retry",
            "run `cargo xtask dev` to expose it to your iPad"
        )
    );
    trunk_in(
        &app_dir(root),
        &[
            "serve",
            "--address",
            "127.0.0.1",
            "--port",
            &PORT.to_string(),
        ],
    )
}

/// `cargo xtask dev [--https]`: serve on every interface and print the LAN URL.
pub fn serve_lan(root: &Path, https: bool) -> Result<(), String> {
    ensure_trunk()?;
    ensure_wasm_target()?;
    let app = app_dir(root);
    let port = PORT.to_string();
    let mut args = vec!["serve", "--address", "0.0.0.0", "--port", port.as_str()];
    let (cert, key) = tls_paths(&app);
    let (cert_s, key_s) = (
        cert.to_string_lossy().into_owned(),
        key.to_string_lossy().into_owned(),
    );
    if https {
        if !cert.is_file() || !key.is_file() {
            return Err(format!(
                "--https needs {} and {}. Create them with mkcert, for example:\n  \
                 mkcert -install\n  mkdir apps/drawcraft-web/dev-cert\n  \
                 mkcert -cert-file apps/drawcraft-web/dev-cert/cert.pem -key-file apps/drawcraft-web/dev-cert/key.pem localhost <your-LAN-IP>\n\
                 then trust the mkcert root CA on the iPad (Settings > General > About > Certificate Trust Settings).",
                cert.display(),
                key.display()
            ));
        }
        args.extend([
            "--tls-cert-path",
            cert_s.as_str(),
            "--tls-key-path",
            key_s.as_str(),
        ]);
    }
    println!("{}", dev_banner(lan_ip(), PORT, https));
    trunk_in(&app, &args)
}

/// `cargo xtask web-build`: release bundle in `apps/drawcraft-web/dist`, then verify it.
pub fn build(root: &Path) -> Result<(), String> {
    ensure_trunk()?;
    ensure_wasm_target()?;
    let app = app_dir(root);
    trunk_in(&app, &["build", "--release"])?;
    let report = verify_dist(&app.join("dist"))?;
    println!("{report}");
    Ok(())
}

/// Checks that `dist` is a complete, installable PWA bundle and summarizes it.
pub fn verify_dist(dist: &Path) -> Result<String, String> {
    let mut missing: Vec<&str> = REQUIRED_DIST_FILES
        .iter()
        .copied()
        .filter(|f| !dist.join(f).is_file())
        .collect();
    let wasm = find_with_suffix(dist, "_bg.wasm");
    let js = find_with_suffix(dist, ".js")
        .into_iter()
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("drawcraft_app") && !n.contains("_bg"))
        })
        .collect::<Vec<_>>();
    if wasm.is_empty() {
        missing.push("drawcraft_app_bg.wasm");
    }
    if js.is_empty() {
        missing.push("drawcraft_app.js");
    }
    if !missing.is_empty() {
        return Err(format!(
            "dist is incomplete, missing: {}",
            missing.join(", ")
        ));
    }
    let wasm_bytes = wasm
        .first()
        .and_then(|p| std::fs::metadata(p).ok())
        .map_or(0, |m| m.len());
    let html = std::fs::read_to_string(dist.join("index.html")).map_err(|e| e.to_string())?;
    for needle in [
        "viewport-fit=cover",
        "apple-mobile-web-app-capable",
        "manifest.webmanifest",
        "touch-action: none",
    ] {
        if !html.contains(needle) {
            return Err(format!("index.html lost a required iPadOS tag: {needle}"));
        }
    }
    Ok(format!(
        "dist ok: {} ({:.1} MiB wasm). Serve it over HTTPS to install it as a PWA.",
        dist.display(),
        wasm_bytes as f64 / (1024.0 * 1024.0)
    ))
}

fn find_with_suffix(dir: &Path, suffix: &str) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.ends_with(suffix))
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn private_lan_detection_covers_rfc1918_link_local_and_ula() {
        for ip in [
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20)),
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 7)),
            IpAddr::V4(Ipv4Addr::new(172, 16, 5, 5)),
            IpAddr::V4(Ipv4Addr::new(169, 254, 1, 1)),
            IpAddr::V6(Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 1)),
        ] {
            assert!(is_private_lan(ip), "{ip}");
        }
        assert!(!is_private_lan(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))));
        assert!(!is_private_lan(IpAddr::V6(Ipv6Addr::LOCALHOST)));
    }

    #[test]
    fn banner_shows_the_lan_url_and_the_secure_context_caveat() {
        let ip = Some(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20)));
        let http = dev_banner(ip, 8080, false);
        assert!(http.contains("http://192.168.1.20:8080"));
        assert!(http.contains("not a secure context"));
        let https = dev_banner(ip, 8080, true);
        assert!(https.contains("https://192.168.1.20:8080"));
        assert!(https.contains("service worker, OPFS"));
        assert!(dev_banner(None, 8080, false).contains("no network address"));
        let odd = dev_banner(Some(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))), 8080, false);
        assert!(odd.contains("not a private LAN address"));
    }

    #[test]
    fn dist_verification_names_every_missing_file() {
        let dir = std::env::temp_dir().join(format!("drawcraft_dist_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(std::fs::create_dir_all(dir.join("icons")).is_ok());
        let err = verify_dist(&dir).err().unwrap_or_default();
        assert!(
            err.contains("index.html") && err.contains("drawcraft_app_bg.wasm"),
            "{err}"
        );

        for f in REQUIRED_DIST_FILES {
            assert!(std::fs::write(dir.join(f), b"x").is_ok());
        }
        assert!(std::fs::write(dir.join("drawcraft_app_bg.wasm"), vec![0u8; 2048]).is_ok());
        assert!(std::fs::write(dir.join("drawcraft_app.js"), b"//").is_ok());
        let err = verify_dist(&dir).err().unwrap_or_default();
        assert!(err.contains("required iPadOS tag"), "{err}");

        let html = "<meta name=\"viewport\" content=\"viewport-fit=cover\"><meta name=\"apple-mobile-web-app-capable\"><link rel=\"manifest\" href=\"manifest.webmanifest\"><style>touch-action: none</style>";
        assert!(std::fs::write(dir.join("index.html"), html).is_ok());
        let ok = verify_dist(&dir).unwrap_or_default();
        assert!(ok.starts_with("dist ok"), "{ok}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tls_files_live_next_to_the_web_app() {
        let (cert, key) = tls_paths(Path::new("apps/drawcraft-web"));
        assert!(cert.ends_with("dev-cert/cert.pem") || cert.ends_with("dev-cert\\cert.pem"));
        assert!(key.ends_with("dev-cert/key.pem") || key.ends_with("dev-cert\\key.pem"));
    }
}
