# DrawCraft Web (PWA)

The Pure-Rust WebAssembly app: the same `drawcraft-ui` as the desktop shell, running in the
browser. The only JavaScript is browser glue: `index.html`, `sw.js` (offline shell) and
`storage_adapter.js` (OPFS / IndexedDB). No application logic lives in JS.

## Commands

| Command | What it does |
|---|---|
| `cargo xtask web` | Serve on `http://localhost:8080` (needs `trunk`). |
| `cargo xtask dev` | Serve on every interface and print the URL to open on an iPad. |
| `cargo xtask dev --https` | Same over HTTPS (needs `dev-cert/cert.pem` and `key.pem`, see below). |
| `cargo xtask web-build` | Release bundle in `apps/drawcraft-web/dist/`, then checks it is complete. |

One-time setup: `rustup target add wasm32-unknown-unknown` and `cargo install trunk --locked`.

## Test on an iPad in Safari

1. Put the computer and the iPad on the same Wi-Fi.
2. Run `cargo xtask dev`. Open the printed `http://<your-ip>:8080` in Safari on the iPad.
3. If the page does not load, allow `trunk` through the computer's firewall (private networks).

**Plain HTTP on a LAN address is not a secure context.** Safari then turns off the service
worker and OPFS. The app still runs and saves to IndexedDB, but "Add to Home Screen" works as a
bookmark only and there is no offline start. To test the real PWA use HTTPS:

```powershell
mkcert -install
mkdir apps/drawcraft-web/dev-cert
mkcert -cert-file apps/drawcraft-web/dev-cert/cert.pem -key-file apps/drawcraft-web/dev-cert/key.pem localhost <your-LAN-IP>
cargo xtask dev --https
```

Install the mkcert root CA on the iPad (AirDrop the `rootCA.pem`, then Settings > General >
VPN & Device Management, and enable it under Settings > General > About > Certificate Trust
Settings). Keep `dev-cert/` out of git.

## Hardware checklist (not verifiable without a device)

- Apple Pencil draws; pressure and tilt change the stroke; double-tap / barrel actions work.
- Resting the palm while writing leaves no marks (Pencil-only is the default).
- One finger pans, two fingers pinch-zoom, a quick two-finger tap undoes, three-finger tap redoes.
- Settings drawer (S) > Writing and reading: "Draw with finger as well" lets one finger draw.
- Magic Keyboard: Cmd+Z / Cmd+Shift+Z, Cmd+K, Cmd+F, Cmd+0, Cmd+9, keys 1-7, P, S, Escape.
  Safari must not open Find-on-page or zoom the page.
- Trackpad: two-finger scroll pans, pinch zooms the page canvas (not the whole Safari page).
- Draw, wait for "Saved", force-quit Safari, reopen: the notebook is still there.
- Airplane mode (HTTPS install only): the installed app starts and works.
- Fill the storage quota (or use a private tab): the banner offers "Export notebook".
