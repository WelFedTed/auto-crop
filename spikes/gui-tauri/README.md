# GUI-stack spike (ROADMAP M0.56 to M0.64, ADR for M0.70)

Throwaway Tauri 2.12 + Svelte 5 prototype. It lives outside the shipping workspace (own `[workspace]`),
is not built by default CI, and is **not** the app shell: the shell is `crates/shell` and `ui/` in M3,
which waits for the M0.70 verdict.

## What it contains

- Rust serves a synthetic 1600x1200 PNG through the `acimg` custom scheme under a random 128-bit
  launch token (`no-store`, `nosniff`, 8 MB cap, strict path parser, never a filesystem path), under
  the CSP and the plugin-free `main` capability of PLAN 8.6.4.
- The webview draws the view (CSS `matrix3d` pan and zoom, wheel, pinch and drag through Pointer
  Events), the quad overlay (corner, edge and grip handles, each a focusable button with a 48 px hit
  area around a 16 px dot, keyboard nudging), a result inset driven by a four-point homography
  (`ui/src/homography.ts`, unit-tested) with no IPC during a drag, and a frame-time HUD.
- "Scripted drag, 10 s" moves a handle for 10 s and prints median and p95 frame time as JSON in the
  spike protocol's shape (docs/adr/spikes.md).

## Run it

```sh
cd spikes/gui-tauri/ui && npm ci && npm test && npm run build
cd .. && cargo run --release --features custom-protocol   # embedded UI, acimg scheme
# UI only, in any browser (synthetic image, no Rust): npm --prefix ui run dev
```

## Status

- Verified: homography unit tests, `svelte-check`, production build, and the UI in a browser (drag,
  perspective preview, zoom, fit). The release build starts on Windows 11 (WebView2 154), the
  webview loads the image through `acimg` (1600x1200), and the scheme refuses a wrong token, `..`,
  an extra segment and a non-zero item id (checked with `<img>` loads over the DevTools protocol;
  `fetch` is blocked by the CSP `connect-src`, as intended).
- Informational only, not a gate cell: a scripted 10 s drag in the Tauri window on the author's
  Windows desktop (60 Hz, 1280x811, 2 MP-class proxy, mouse-class input) gave 600 samples, median
  16.7 ms and p95 16.8 ms, i.e. vsync-bound. It says nothing about the 24 MP workload, touch or the
  Surface.
- **Not measured yet (every cell UNMEASURED):** frame times on WebView2, WKWebView and WebKitGTK,
  touch and pen, Linux pinch, HiDPI and dark mode, tile and IPC plumbing, drop fixtures. A browser
  pane throttles `requestAnimationFrame`, so numbers from it are not valid; run the scripted drag in
  the real Tauri window on the device under test.
- Not built yet: 24 MP pyramid and tiles, the loupe, the angle ruler, the isolation-pattern IPC
  comparison, the probe for software rendering on Linux.
