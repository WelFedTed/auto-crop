# Research: GUI framework and desktop shell  (key: gui)

## Summary
Nine stacks were scored on ten weighted criteria. Tauri 2 with a Svelte/TypeScript frontend comes out ahead (about 82/100). Electron is close (about 80) but is heavy, which conflicts with "very fast". Slint, Qt via cxx-qt, egui and Flutter cluster at 76-77, which is within scoring noise. Tauri leads because it has the best packaging and updater story, a permissive licence, the largest contributor pool, web-grade accessibility/IME/i18n, and Rust-native raw-byte IPC. Its main weakness is Linux: WebKitGTK has documented GPU and pinch-zoom quirks, and I could not verify touch behaviour on current WebKitGTK. Slint is the best fallback because it is native, has built-in cross-platform pinch/rotate gestures, and needs no webview. Keep the image pipeline in a UI-agnostic Rust core crate so the shell can be swapped. Tauri 3 is in alpha (2026-09-13), including an optional Chromium (CEF) runtime, so start on the stable 2.x line.

## Recommendation
Primary: Tauri 2.12 (stable), with a Svelte 5 + TypeScript frontend and a UI-agnostic `auto-crop-core` Rust crate. Rust renders preview bitmaps and tiles and serves them over a custom URI scheme. The webview only draws the image, the overlay handles, the loupe and the gesture layer. Fallback: Slint, which keeps everything in Rust and has a native pinch/rotate handler. Switch to Slint if a Week-1 spike shows WebKitGTK cannot pinch/pan a 24 MP proxy smoothly with touch on Linux (Wayland+NVIDIA and X11+Intel). Slint's costs are GPL-3.0-or-attribution licensing and a young OS drag-and-drop implementation. Tauri's alpha CEF runtime is a watch item for making Linux match Windows, at Electron-like size. Rust as the backend language is right. The rendering choice is what drives the risk.

## Key findings
- Tauri 2.12.0 is the latest stable (2026-09-26, releases roughly every 1-3 months). Tauri 3.0.0-alpha.0-3 shipped 2026-09-13 to 09-26. Tauri 3 makes the runtime explicit (tauri-runtime-wry or tauri-runtime-cef) and feature-gates gtk3/gtk4. I found no evidence of a WebKitGTK 6/GTK4 webview.
- Tauri on Linux renders through WebKitGTK. Tauri's own docs describe NVIDIA/DMABUF blank windows and Wayland Error 71, and warn that WebGL/canvas can silently fall back to slow paths. The workarounds are env vars set in main().
- In Tauri, pinch zoom on Linux is handled by GTK and ignores in-page handling. The only workaround is unsafe with_webview code (discussion thread runs 2022-2024, so may be stale). WebView2 has IsPinchZoomEnabled (default true). macOS pinch/rotate in WKWebView arrives only as non-standard WebKit gesture* events.
- Tauri gives drag-and-drop with file paths and positions, plus a dialog plugin with directory and multi-select. Raw bytes go through tauri::ipc::Response or Channels, or custom URI schemes. The Tauri updater works on Windows, macOS and Linux AppImage only, and signing is mandatory.
- Slint 1.18.1 (2026-09-21) ships minors every ~2-3 months. ScaleRotateGestureHandler arrived in 1.14 (raw two-finger touch on Windows/Linux, trackpad gestures on macOS/iOS). DragArea/DropArea arrived in 1.17, with DataTransfer::file_paths() available. Skia is the default renderer since 1.16, and wgpu texture import is still unstable-*. Slint has no built-in file dialogs (use rfd).
- Slint's licence is GPL-3.0-only OR Royalty-free-2.0 OR commercial. The royalty-free option requires an AboutSlint widget or a download-page badge and excludes embedded use. In practice the app is GPLv3, or permissive with attribution.
- egui/eframe 0.36.2 (2026-09-08) is pre-1.0, has MultiTouchInfo, and enables AccessKit by default. Stable winit docs list Pinch/Rotation gestures on macOS/iOS only (Wayland support landed for 0.31, beta.3 on 2026-09-04). Iced 0.14.0 (2025-12-07) has had no release since, and its accessibility support lives in a fork.
- Qt is the strongest touch toolkit (PinchHandler, MultiPointTouchArea), but cxx-qt calls itself early development with frequent API changes (0.10.0, 2026-08-24). Flutter 3.47 (2026-08-12) with flutter_rust_bridge 2.13 (Flutter Favorite) is strong on gestures but adds Dart, and folder drop via desktop_drop is unverified.

## Risks
- Linux WebKitGTK is the top risk: GPU and DMABUF driver problems, page zoom hijacking pinch, and distro-dependent versions. A test matrix of Ubuntu 24.04 and Fedora on Wayland and X11, with NVIDIA and Intel, is needed before committing.
- I could not verify current WebKitGTK 2.4x touch and Pointer Events behaviour on Wayland and X11 (search budget exhausted). The Week-1 spike must measure it.
- Tauri 3 is alpha, and the length of the 2.x maintenance window is unknown. Pin to 2.x and isolate Tauri-specific code in a thin shell crate.
- Webview limits: WebGL and canvas texture sizes are around 8-16k px and the webview cannot share a GPU texture with Rust. Never send full-resolution images. Send a 2-4 MP proxy plus tiles.
- Webview HEIC/HEIF and RAW decoding is unreliable. Rust must decode everything and hand the UI only JPEG/WebP/PNG.
- Do not build a GLSL preview and a separate Rust implementation of warp/enhance, or they will drift apart. Use one Rust implementation for previews.
- Distribution: Windows and macOS signing/notarization cost and effort (Apple Developer Program fee), and a Linux updater limited to AppImage (prefer Flatpak plus the store for updates).
- Slint fallback risks: GPL/attribution licensing, brand-new OS drag-and-drop, unstable wgpu API tied to wgpu major versions, and stale cargo-packager (0.11.8, Nov 2025). Size, memory and startup figures in this report are my estimates, not measurements.

## Options evaluated

### Tauri 2 (+ Svelte 5/TypeScript) — recommended
Rust core with the OS webview (WebView2, WKWebView, WebKitGTK) and a web frontend.
- licence: MIT OR Apache-2.0
- status: 2.12.0 (2026-09-26). Tauri 3 alpha.3 (2026-09-26). wry 0.57.0 (2026-09-08).
- pros: Raw-byte IPC, Channels and custom URI schemes carry previews and tiles; MSI/NSIS/dmg/deb/rpm/AppImage/Flatpak bundles and a signed updater; Path-bearing drag-drop events, folder dialog, file associations; Web-grade accessibility, IME, i18n, HiDPI and dark mode; Largest contributor pool of the Rust options
- cons: WebKitGTK GPU, touch and pinch quirks on Linux; Three different webview engines to test; No shared GPU texture, so previews are bitmap or tile transfers; Updater on Linux is AppImage-only; Tauri 3 alpha churn

### Slint — fallback
Declarative native GPU toolkit (.slint DSL) driven from Rust.
- licence: GPL-3.0-only OR Royalty-free-2.0 (attribution, no embedded) OR commercial
- status: 1.18.1 (2026-09-21). Minor releases every 2-3 months.
- pros: ScaleRotateGestureHandler and SwipeGestureHandler are built in; Skia default renderer; wgpu texture import (unstable); Small, fast, low memory; no webview; DragArea/DropArea with DataTransfer::file_paths(); AccessKit and gettext i18n
- cons: OS file/folder drop is new (1.16-1.17); No built-in file dialogs (use rfd) and no tiled viewer widget; Packaging and updater are DIY; GPLv3 or attribution constraint on the app licence

### egui / eframe — viable
Immediate-mode Rust GUI on winit and wgpu.
- licence: MIT OR Apache-2.0
- status: 0.36.2 (2026-09-08). Pre-1.0, breaking release about every 2 months.
- pros: Pure Rust with full wgpu control via paint callbacks; MultiTouchInfo gives zoom, rotation and translation; AccessKit on by default; IME improved in 0.35
- cons: Widgets are not touch-first and the look is developer-tool style; No trackpad pinch on Windows or Linux from stable winit; No i18n; custom canvas is invisible to screen readers

### Qt 6 + cxx-qt (QML) — viable
Qt Quick UI with Rust QObjects exposed through cxx-qt.
- licence: Qt LGPL-3.0 (dynamic link); cxx-qt MIT OR Apache-2.0
- status: Qt 6.11.x (6.11.0 on 2026-03-23; 6.12 beta; 6.8 LTS). cxx-qt 0.10.0 (2026-08-24).
- pros: Best touch/gesture toolkit (PinchHandler, MultiPointTouchArea); Native dialogs, DropArea, top-tier accessibility, IME and i18n; Mature scene graph
- cons: cxx-qt says early development with frequent API changes; C++/CMake toolchain, likely C++ shim for custom items; LGPL relink obligations; larger deployment (est. 25-60 MB)

### Flutter + flutter_rust_bridge — viable
Dart/Flutter UI with Rust called through generated FFI.
- licence: BSD-3 (Flutter); MIT (FRB)
- status: Flutter 3.47 (2026-08-12). FRB 2.13.0 (Flutter Favorite).
- pros: Excellent scale/rotate gestures and consistent rendering, no webview; Hot reload and a big ecosystem; Built-in magnifier widget
- cons: Second language (Dart) plus codegen; Folder drop via third-party desktop_drop (0.8.4); folder support not verified; Linux embedder is the least polished target; no first-party installers

### Dioxus desktop — viable
Rust RSX UI rendered in a wry webview.
- licence: MIT OR Apache-2.0
- status: 0.7.10 stable (2026-07-30); 0.8.0-alpha.1. dioxus-native is experimental.
- pros: All-Rust UI code; Same webview capabilities as Tauri
- cons: Inherits every WebKitGTK and pinch-zoom problem; Smaller ecosystem and less packaging/updater tooling than Tauri; Loses most of the JS viewer and UI libraries

### Electron (+ napi-rs) — viable
Bundled Chromium plus Node, with Rust exposed as a native addon.
- licence: MIT
- status: 44.5.1
- pros: Consistent Chromium touch, pointer and GPU on every OS; Mature packaging and updater; Path-bearing drops and native dialogs
- cons: Roughly 100+ MB installer and high RAM (estimate); Node/N-API bridge instead of a single Rust binary; Conflicts with 'very fast' footprint

### Iced; GTK4/libadwaita (gtk-rs) — avoid
Two Rust-native toolkits grouped here because both fit this project poorly.
- licence: Iced MIT; GTK4 LGPL-2.1+
- status: Iced 0.14.0 (2025-12-07, no release since). gtk4-rs 0.11.5 (2026-09-20).
- pros: Iced: pure Rust, wgpu shader widget; GTK4: excellent gestures and Flatpak story on Linux
- cons: Iced: raw finger events only, no gesture recognition, accessibility only in a fork; GTK4: Windows and macOS are second-class for packaging and accessibility (my assessment, not source-verified)

## Deliverable
**Scores** are 1-5 (5 = best), my judgement from the cited sources. Weights: a18 b14 c6 d8 e10 f8 g8 h10 i10 j8. Differences under about 5 points are noise.

Criteria: a = touch and gestures; b = large-image viewer and overlays; c = drag-and-drop and native dialogs; d = accessibility/HiDPI/dark mode/i18n/IME; e = startup/size/memory; f = packaging and auto-update; g = licence fit for an open-source app; h = maturity and cadence; i = pitfall exposure (5 = few); j = Rust integration and contributor reach.

| Stack | a | b | c | d | e | f | g | h | i | j | /100 |
|---|---|---|---|---|---|---|---|---|---|---|---|
| **Tauri 2 + Svelte** | 3.5 | 3.5 | 5 | 4.5 | 4 | 5 | 5 | 5 | 2.5 | 4.5 | **82** |
| Electron + napi-rs | 4 | 4.5 | 5 | 4.5 | 1.5 | 4.5 | 5 | 5 | 3.5 | 3 | 80 |
| **Slint** | 4 | 3.5 | 3.5 | 4 | 4.5 | 3 | 3.5 | 4.5 | 3.5 | 4 | 77 |
| Qt6 + cxx-qt | 5 | 4 | 5 | 5 | 3 | 3.5 | 3.5 | 3.5 | 3 | 2 | 77 |
| egui/eframe | 3 | 4 | 4 | 3 | 4.5 | 3 | 5 | 4 | 3.5 | 5 | 76 |
| Flutter + FRB | 4.5 | 4 | 3.5 | 4 | 3 | 3 | 5 | 4 | 3 | 3 | 76 |
| Dioxus desktop | 3.5 | 3.5 | 4 | 4 | 4 | 3.5 | 5 | 3.5 | 2.5 | 4.5 | 74 |
| Iced | 2 | 3.5 | 3.5 | 2 | 4.5 | 3 | 5 | 3 | 3 | 5 | 66 |
| GTK4-rs | 3 | 3.5 | 4 | 2.5 | 3 | 2.5 | 4.5 | 4 | 2 | 3.5 | 64 |

**Why the odd scores**
- Tauri touch (a) is high on Windows and macOS and weak on Linux (WebKitGTK). Its pitfall score (i) is lowest for the same reason.
- Electron scores well everywhere except size and memory, which is why it is a baseline and not a pick.
- Qt wins touch but is dragged down by cxx-qt maturity and the C++/CMake toolchain.
- egui gets no trackpad pinch on Windows or Linux from stable winit. Iced has raw finger events only.

**Recommended rendering architecture (Tauri)**
1. `auto-crop-core` is a pure Rust crate. It decodes (Rust handles HEIC and RAW), detects the page quad, warps, crops, enhances and encodes. Edits are stored as a parametric edit list, so undo and redo are tiny history entries and never pixel snapshots.
2. On open, Rust builds a 2-4 MP sRGB proxy and lazily builds 512 px tiles. A custom URI scheme serves them as high-quality JPEG or WebP with a version counter.
3. Overlay geometry is drawn in the webview in image coordinates with SVG or canvas, so there is no IPC while dragging. This covers the crop quad, rotation handle, and 24-32 px touch slop targets.
4. Gestures use Pointer Events with `touch-action: none`, plus a small two-pointer pinch/pan/rotate recognizer. macOS also gets `gesture*` events. Linux needs a WebKitGTK gesture-zoom disable shim.
5. The loupe is a second canvas that samples the proxy or a tile at 4-6x, offset from the finger.
6. On pointer-up, the webview sends the params to Rust, which renders the warped and enhanced proxy. There is a single implementation, and the result cross-fades in. Full-resolution export runs in a worker with a progress channel and cancellation.
7. A shared GPU texture is impossible across the webview boundary. The Slint fallback can pass a `SharedPixelBuffer`, or a wgpu texture through the unstable API.

**Week-1 gates**
- 24 MP proxy pinch/pan/rotate at ~60 fps on a Surface, a MacBook trackpad, and Ubuntu 24.04 Wayland (NVIDIA) and X11 (Intel).
- A dropped folder of 500 images.
- HiDPI and dark mode.
- A signed installer plus an update round-trip.
- If Linux fails, switch to Slint.

## Decision-critical claims (as researched)
- Tauri 2.12.0 is the latest stable release (2026-09-26). Tauri 3.0.0-alpha.3 was published the same day, and a tauri-runtime-cef 3.0.0-alpha.4 crate exists. [https://crates.io/api/v1/crates/tauri/versions?per_page=15]
- Tauri 3 alpha requires selecting tauri-runtime-wry or tauri-runtime-cef explicitly, and makes GTK support feature-gated (gtk3 or gtk4). [https://github.com/tauri-apps/tauri/releases/tag/tauri-v3.0.0-alpha.0]
- On Linux, Tauri renders through WebKitGTK. The official docs describe blank windows, DMABUF and Wayland Error 71 problems (mostly NVIDIA), with env-var workarounds, and warn that WebGL/canvas can silently degrade to slow paths. [https://v2.tauri.app/develop/debug/linux-graphics/]
- Touchpad pinch zoom in WebKitGTK is handled by GTK and ignores in-page handling. The workaround is unsafe with_webview code, while WebView2 exposes IsPinchZoomEnabled (default true). [https://github.com/tauri-apps/tauri/discussions/3843]
- Tauri's onDragDropEvent delivers dropped paths and positions. Native drag-drop must be disabled for HTML5 DnD on Windows. The dialog plugin supports directory and multiple selection. The updater supports Windows, macOS and Linux AppImage only, with mandatory signing. [https://v2.tauri.app/reference/javascript/api/namespacewebview/]
- Slint's ScaleRotateGestureHandler (since 1.14) does pinch and rotate from raw two-finger touch on all platforms and from trackpad gestures on macOS/iOS. DragArea/DropArea landed in 1.17, with file paths in DataTransfer since 1.16. [https://docs.slint.dev/latest/docs/slint/reference/gestures/scalerotategesturehandler/]
- Slint is licensed GPL-3.0-only OR Royalty-free-2.0 OR commercial. The royalty-free licence requires an AboutSlint widget or a download-page badge and excludes embedded systems. Latest is 1.18.1 (2026-09-21). [https://crates.io/api/v1/crates/slint]
- Stable winit documents PinchGesture and RotationGesture as macOS/iOS only, and Touch as unsupported on macOS. Wayland pinch/rotation support arrived via PR 3656 for the 0.31 line, which is still beta. This limits egui and Iced trackpad gestures on Windows and Linux. [https://docs.rs/winit/latest/winit/event/enum.WindowEvent.html]

## Researcher questions for user
- Which licence do you want for Auto Crop? — It decides whether Slint is a usable fallback without attribution obligations, and how easily others can reuse or fork the code. (default: MIT OR Apache-2.0. With Tauri as primary nothing forces copyleft, and if you fall back to Slint under that licence you show the AboutSlint attribution.)
- How first-class must Linux touch and gestures be at v1? — WebKitGTK is Tauri's weak spot, so this decides how much time goes into the Linux spike and how quickly you switch to Slint. (default: First-class for mouse, trackpad and keyboard. Touch on Linux is validated in the spike, and it triggers the Slint switch if it fails.)
- Might you want Android or iOS versions (for example, photographing receipts with the phone camera)? — Tauri 2 targets mobile and Slint does too, which changes how you structure the UI. The core crate stays reusable either way. (default: Possibly later. Keep the core UI-agnostic and the touch targets large, and do not build mobile yet.)
- If Tauri on Linux disappoints, which trade-off would you accept? — It defines the pre-agreed escape route so the decision is not re-litigated mid-project. (default: Switch UI to Slint, keeping the entire core crate.)

## INDEPENDENT VERIFICATION (skeptic) — overrides the researcher where they differ
- [CONFIRMED] 1. Tauri 2.12.0 is latest stable (2026-09-26); Tauri 3.0.0-alpha.3 published same day; tauri-runtime-cef 3.0.0-alpha.4 exists
  CORRECTION: All three hold. crates.io shows tauri 2.12.0 and 3.0.0-alpha.3 both dated 2026-09-26, and tauri-runtime-cef has 3.0.0-alpha.4 (2026-09-26); it skipped alpha.3. Tauri 3 alpha.0 was 2026-09-13. wry 0.57.0 (2026-09-08) also checks out.
- [CONFIRMED] 2. Tauri 3 alpha requires explicit tauri-runtime-wry or tauri-runtime-cef and feature-gates GTK (gtk3 or gtk4)
  CORRECTION: The alpha.0 notes say the runtime is chosen via Builder::runtime(...) with a runtime crate instead of cargo features, and GTK bindings come from new gtk3/gtk4 features. Caveat: wry 0.57.0 still pins webkit2gtk 2.0.2 (GTK3 WebKitGTK), so this does not yet mean a WebKitGTK 6 or GTK4 webview.
- [CONFIRMED] 3. Linux Tauri uses WebKitGTK; docs describe blank windows, DMABUF, Wayland Error 71 (mostly NVIDIA), env-var workarounds, and silent WebGL/canvas slow paths
  CORRECTION: The page matches, including a section titled 'Silent failures: WebGL and canvas'. It notes WebGL2 creation succeeds on a software rasterizer and that WebKitGTK masks the renderer string. Env vars: __NV_DISABLE_EXPLICIT_SYNC, WEBKIT_DISABLE_DMABUF_RENDERER, WEBKIT_DISABLE_COMPOSITING_MODE.
- [PARTLY-TRUE] 4. WebKitGTK touchpad pinch zoom is GTK-handled and ignores in-page handling (workaround = unsafe with_webview); WebView2 exposes IsPinchZoomEnabled (default true)
  CORRECTION: The Linux half holds: wry#544 and tauri#13115 are still open, with no built-in fix. The Windows half is misleading. wry maps its zoom_hotkeys_enabled attribute to SetIsPinchZoomEnabled and defaults it to false, so Tauri already disables pinch page-zoom on WebView2 by default, with no with_webview code. That flag is documented as unsupported on macOS and Linux. WebView2's own default of true is confirmed but overridden by wry.
- [PARTLY-TRUE] 5. onDragDropEvent gives paths+positions; native drag-drop must be disabled for HTML5 DnD on Windows; dialog supports directory+multiple; updater supports Windows, macOS and Linux AppImage only, signing mandatory
  CORRECTION: Drag-drop (paths plus physical-pixel position, dragDropEnabled must be off for HTML5 DnD on Windows), dialog (directory and multiple on desktop) and mandatory signing are confirmed. 'Linux AppImage only' is stale. tauri-plugin-updater 2.10.0 added Deb and RPM (plus AppImage) on Linux, and NSIS/MSI on Windows. The source has install_deb and install_rpm, and the latest plugin is 2.13.1 (2026-09-29). The docs page still only lists AppImage artifacts.
- [REFUTED] 6. Slint ScaleRotateGestureHandler (since 1.14) does pinch/rotate from touch on all platforms and trackpad on macOS/iOS; DragArea/DropArea in 1.17, file paths in DataTransfer since 1.16
  CORRECTION: The platform statement is confirmed: touchscreens on all platforms, trackpad on macOS and iOS only. The versions are wrong. The changelog lists ScaleRotateGestureHandler under 1.16.0 (2026-04-16), not 1.14. DragArea/DropArea arrived in 1.17.0 (2026-06-24) for in-window drag and drop. File paths in DataTransfer landed in 1.18.0 (2026-09-16, PR 12550 merged 2026-07-22), not 1.16. Decision-critical: OS file/folder drops from a file manager are implemented only in the Qt backend. The default winit backend (default since 1.16) has no OS drop wiring. The winit 0.31 DnD API merged upstream on 2026-07-16, but 0.31 is still beta and Slint's winit-0.31 branch is in progress. The workaround is on_winit_window_event with winit 0.30 DroppedFile events, which I did not verify on Wayland.
- [CONFIRMED] 7. Slint licence is GPL-3.0-only OR Royalty-free-2.0 OR commercial; royalty-free needs AboutSlint widget or download-page badge and excludes embedded; latest 1.18.1 (2026-09-21)
  CORRECTION: crates.io licence string is GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0. Slint's terms page states the attribution requirement (AboutSlint or a download-page badge) and the embedded exclusion. 1.18.1 is dated 2026-09-21 in both the changelog and crates.io.
- [PARTLY-TRUE] 8. Stable winit documents PinchGesture/RotationGesture as macOS/iOS only and Touch as unsupported on macOS; Wayland pinch/rotation via PR 3656 for 0.31 (still beta); limits egui and Iced trackpad gestures on Windows/Linux
  CORRECTION: winit 0.30.13 docs confirm pinch and rotation as macOS/iOS only and Touch as unsupported on macOS. PR 3656 was closed unmerged (2025-09-07). The Wayland support that shipped is PR 4338 (merged 2025-09-07). 0.31.0-beta.3 (2026-09-04) is still the newest 0.31 release. Its docs list pinch and rotation for macOS, iOS and Wayland only, so Windows and X11 still have no trackpad pinch. The conclusion for egui and Iced stands.

### Other errors spotted by skeptic
- Slint option lists 'Skia default renderer'. Slint's Cargo feature docs say renderer-femtovg is the default. Skia is opt-in via renderer-skia (winit-skia).
- Tauri pros list 'Flatpak bundles'. Tauri's bundler does not produce Flatpak. The docs describe a manual flatpak-builder manifest built from the .deb, and the bundle-as-Flatpak feature request (tauri#3619) is still open.
- Tauri cons say 'Updater on Linux is AppImage-only'. That is stale: updater plugin 2.10.0+ handles deb, rpm and AppImage. Snap/Flatpak installs would be updated by their own stores.
- Fallback rationale is weaker than presented. Slint's default winit backend has no OS file or folder drop in 1.18.1 (Qt backend only), and the app requires drag-and-drop of files and folders. The Slint c (drag-drop and dialogs) score of 3.5 looks generous. The Week-1 gates should test a folder drop on the Slint path too, not just WebKitGTK touch.
- Qt row says '6.12 beta'. Qt's release-support page now lists 6.12 as supported (next LTS, support until 2031-09-30), and 6.11.2 (2026-08-18) is the latest 6.11 patch. I could not confirm a 6.12.0 tarball on download.qt.io (404), so treat the 6.12 status as uncertain rather than beta.
- Flutter is at 3.47.5 (2026-09-18), FRB stable is 2.13.0 with 2.14 betas out, and Electron 44.5.1 was published today (2026-09-30). The listed versions are otherwise consistent with primary sources (egui 0.36.2, cxx-qt 0.10.0, gtk4 0.11.5, iced 0.14.0 with no later release, dioxus 0.7.10 and 0.8.0-alpha.1, desktop_drop 0.8.4). The weighted score arithmetic reproduces (82/80/77/77/76/76/74/66/64).
- The deliverable's 'Rust handles HEIC and RAW' is outside this GUI review and unchecked. HEIC decoding normally needs libheif (a C library, with HEVC patent considerations), so it should be verified in the codec research.

## Sources
- https://crates.io/api/v1/crates/tauri/versions?per_page=15
- https://crates.io/api/v1/crates/tauri-runtime-cef
- https://crates.io/api/v1/crates/wry/versions?per_page=8
- https://github.com/tauri-apps/tauri/releases/tag/tauri-v3.0.0-alpha.0
- https://v2.tauri.app/blog/
- https://v2.tauri.app/develop/debug/linux-graphics/
- https://github.com/tauri-apps/tauri/discussions/3843
- https://v2.tauri.app/reference/javascript/api/namespacewebview/
- https://v2.tauri.app/plugin/dialog/
- https://v2.tauri.app/plugin/updater/
- https://v2.tauri.app/distribute/
- https://v2.tauri.app/develop/calling-rust/
- https://v2.tauri.app/reference/webview-versions/
- https://v2.tauri.app/start/
- https://docs.rs/tauri/latest/tauri/webview/struct.WebviewBuilder.html
- https://learn.microsoft.com/en-us/microsoft-edge/webview2/reference/win32/icorewebview2settings5
- https://developer.mozilla.org/en-US/docs/Web/API/Element/gesturechange_event
- https://developer.mozilla.org/en-US/docs/Web/API/Pointer_events/Multi-touch_interaction
- https://crates.io/api/v1/crates/slint
- https://raw.githubusercontent.com/slint-ui/slint/master/CHANGELOG.md
- https://docs.slint.dev/latest/docs/slint/reference/gestures/scalerotategesturehandler/
- https://docs.rs/slint/latest/slint/struct.DataTransfer.html
- https://docs.slint.dev/latest/docs/slint/guide/backends-and-renderers/backends_and_renderers/
- https://app.unpkg.com/slint-ui@1.12.0/files/LICENSES/LicenseRef-Slint-Royalty-free-2.0.md
- https://crates.io/api/v1/crates/eframe
- https://docs.rs/egui/latest/egui/struct.MultiTouchInfo.html
- https://raw.githubusercontent.com/emilk/egui/main/CHANGELOG.md
- https://crates.io/api/v1/crates/winit/versions?per_page=6
- https://docs.rs/winit/latest/winit/event/enum.WindowEvent.html
- https://crates.io/api/v1/crates/iced
- https://github.com/iced-rs/iced
- https://crates.io/api/v1/crates/dioxus-desktop/versions?per_page=4
- https://crates.io/api/v1/crates/cxx-qt/versions?per_page=5
- https://github.com/KDAB/cxx-qt
- https://www.qt.io/development/qt-framework/latest-releases
- https://doc.qt.io/qt-6/license-changes.html
- https://crates.io/api/v1/crates/gtk4/versions?per_page=5
- https://docs.flutter.dev/release/whats-new
- https://pub.dev/packages/flutter_rust_bridge
- https://pub.dev/packages/desktop_drop
- https://registry.npmjs.org/electron/latest
- https://crates.io/api/v1/crates/cargo-packager