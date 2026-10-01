# Research: Touch-first UX design  (key: ux)

## Summary
Auto Crop's UX should be triage-first: import, auto-process everything, then have the user review only what the app is unsure about. Comparable scanner apps (Genius Scan, Adobe Scan, Google Drive, Apple Notes) all follow "auto-detect, then drag four corners", but none targets batches of hundreds on a desktop. ScanTailor Advanced does (stage pipeline, red deviation flags) but is a mouse-era UI. Even the best vendor-reported detector is right about 85% of the time, so calibrated Good/Check/Failed triage, non-destructive edits and cheap undo matter more than any single gesture. Touch design rests on 44-48 px hit targets (visible handles smaller), separate Review mode (swipe) and Adjust mode (handles) to avoid gesture conflicts, a touch-only loupe, and a non-drag alternative for every drag or pinch (WCAG 2.5.1 and 2.5.7). Microsoft Lens was retired in early 2026, leaving a local-first gap.

## Recommendation
Build around three screens: Import (drop zone plus three presets), Batch Review (grid sorted by confidence with "Accept all good") and Edit (canvas with a four-corner quad, angle ruler, enhance chips). Use 44-48 px hit areas everywhere and adapt density to the last-used pointer type, not OS mode (Windows 11 removed tablet mode). Keep originals untouched, treat gestures as accelerators over always-visible buttons, and export through one dialog with a Simple/Advanced split. A Rust core is right. The GUI toolkit is constrained by UX: it needs real multi-touch recognisers, platform accessibility trees, IME and RTL. Slint (gesture handler since 1.16) and Qt Quick fit best. A web UI (Tauri) is fastest to build but has the weakest Linux touch story. Decide with a one-week spike on a Surface, a MacBook trackpad and a Linux touchscreen before committing.

## Key findings
- PRIMARY FLOW. Entry points: drag-drop on the window or dock/taskbar icon, Open files/folder (big buttons, since drag-drop is impractical on touch), Paste, OS 'Open with'. Empty state = large drop zone (tap to browse), a 3-way preset (Receipts/Documents, Photos, Convert only) and 'Try sample images'; no wizard, no modal tutorial. On import, auto-processing starts at once, visible items first, thumbnails streaming in. Batch Review is a grid of processed results (not originals), sorted by confidence and filtered by Needs review / All / Edited / Skipped / Failed. A summary bar offers 'Accept all good (289)' and 'Review 23'. Review is a full-screen result view: swipe or Next/Prev, Accept and next (Enter), Skip (X), Adjust (E). Export is one button with a count. Design target (not measured): 300 images with about 25 flagged reviewed in roughly two minutes.
- AUTO RESULT FIRST, MANUAL SECOND. Two modes avoid gesture conflicts: Review (swipe between images, no handles) and Adjust (source image with an editable quad; swipe disabled). Handles: 16 px visible dot with dual white/dark stroke (3:1 non-text contrast on any photo), 48 px hit area, hit priority corner > edge > body, and a 32 px gutter so handles never sit at the screen edge. Body drag pans; a centre grip moves the whole quad. While a finger drags a handle, a 3-4x loupe appears offset up-left of the finger (Genius Scan does this) and flips near edges. Within about 48 px of the viewport edge, after roughly 250 ms dwell, the view auto-pans (starting values to tune). A small live 'result' inset updates during drags. Compare = momentary press-and-hold (Snapseed/Lightroom mobile) plus a draggable split (Squoosh). Scope selector: This image / Selected / All; customised images show a dot.
- LAYOUT. Use Windows breakpoints (small up to 640, medium 641-1007, large 1008+ epx), which line up with Material compact/medium/expanded classes. Wide: filmstrip left, canvas centre, 320 px inspector right, Accept/Skip bottom-right. Medium or portrait tablet: canvas on top, bottom tab bar (Crop | Rotate | Enhance | More), inspector as bottom sheet. Compact or snapped window: single pane, grid via Back. Thumb zones: tablets are mostly used seated and held at the sides (Hoober), so put frequent actions (Accept, Skip, Undo, 90-degree rotate, ruler) along bottom corners and edges, and rare or risky ones (Export, Settings) top-right. Offer a left/right mirror. Keep controls at least 24 px from screen edges (Windows edge swipes). Adapt density to last-used pointer type (touch/pen/mouse) because Windows 11 has no tablet mode and macOS/Linux give no posture signal.
- UNDO/REDO. Edits are non-destructive parameters (quad, angle, filter settings), so history is cheap and originals are untouched until export. Two stacks: per-image (in Edit; one step per completed drag, not per pointer move) and session-level batch actions (accept all, apply to N, skip). Always-visible 48 px Undo/Redo buttons, with the operation named in tooltip and menu (Apple HIG asks for this), Ctrl/Cmd+Z, Ctrl+Shift+Z or Ctrl+Y, and an Edit menu on macOS. A History drawer lists steps with thumbnails; tap to jump back. 'Reset to auto' is itself undoable. Touch gestures are opt-in: two-finger tap undo, three-finger tap redo (Procreate convention). Avoid three-finger swipes (Windows and iPadOS reserve them) and shake (WCAG 2.5.4 needs a button anyway). Autosave the session (edits plus a capped undo depth) so a crash loses nothing.
- EXPORT/CONVERT DIALOG (a sheet, not a full page). Simple: Format (Same as original, JPG, PNG, WebP, AVIF, TIFF, PDF); Quality presets (Small/Balanced/Best) with live size estimate; Resize (Original, longest edge, percent; never upscale by default); Metadata (Keep but strip location [default], Keep all, Strip all); Save to (folder next to originals '/AutoCrop' [default], Choose folder, Replace originals [Advanced, sends originals to Trash]); Name template ({name}_crop, {n}, {date}) with a live preview of the first three names; If exists (Keep both / Skip / Replace). For HEIC/HEIF input: default output JPG, convert Display P3 to sRGB, bake in EXIF rotation, warn that depth maps, Live Photo motion and HDR gain maps are dropped. The 'Convert only' preset skips detection and opens this dialog directly. Advanced: chroma subsampling, progressive, PNG/TIFF compression, ICC embedding, gain-map handling, DPI, PDF page size, concurrency.
- ACCESSIBILITY, THEME, I18N. Every drag or pinch has a single-pointer non-drag alternative (WCAG 2.5.1, 2.5.7): +/- zoom, numeric fields for angle and corners, arrow-key nudging (Tab to a handle, 1 px; Shift 10 px), and the angle slider follows ARIA slider keys. Targets: 44 px hit area, never below WCAG 2.5.8's 24 px. Screen readers get names with status ('IMG_07, needs review, edges unclear'), live-region progress and handle values; status is never colour-only. Light/dark/system plus a high-contrast theme honouring Windows Contrast Themes; neutral grey canvas so white paper edges stay visible. Honour reduced motion (no slide or inertia). Externalise strings (Fluent or ICU MessageFormat) with a pseudo-locale in CI. RTL: mirror layout, filmstrip order and prev/next arrows, but not the image, the angle ruler or rotation icons; budget 30-40% text expansion. Toolkit must expose UIA, NSAccessibility and AT-SPI (AccessKit or a webview).
- LOW CONFIDENCE AND ERRORS. Show three tiers, icon plus word (Good / Check / Failed), never a raw score. Signals: edge support on all four sides, plausible corner angles and area, competing candidates, blur, content touching the frame, and outliers versus the batch median (ScanTailor's deviation highlighting). Failed detection falls back to 'original, no crop' with an amber banner offering Draw crop and Try another candidate (up to three). Google Drive likewise asks for corner adjustment when unsure. File errors (corrupt, unsupported, missing HEIC codec, too large) go to a non-modal Issues tray with reason, Retry, Show in folder, Copy details; never one modal per file. Export failures pause and resume (disk full, permission, name clash) and end with a summary (309 done, 2 skipped, 1 failed) plus Retry failed. Never overwrite originals by default.
- WHAT TO HIDE. Visible by default: preset, crop Auto/Off, angle, Enhance (Off, Auto, B&W) with one Strength slider, aspect lock, and export format/quality/size/location. Behind Advanced (per section, remembered): detection sensitivity and confidence thresholds, max skew angle, perspective on/off, crop padding, binarisation method and threshold, whiten background, denoise/despeckle, sharpen, DPI, chroma subsampling, ICC and gain-map handling, naming tokens, conflict policy, threads/GPU, cache and undo depth, gesture prefs (loupe, undo taps, edge auto-pan), shortcut rebinding, compact density, Replace originals. Enhance is offered, not forced: a chip 'Looks like a document: enhance?' previews in one tap, and it never auto-applies without one-tap revert.

## Risks
- Gesture parity: web UIs see three engines (Chromium-style ctrl+wheel pinch, Safari's non-standard GestureEvent, WebKitGTK pinch-zoom always on), and native Rust toolkits are young. Mitigate with a week-1 multi-touch spike on Surface, macOS trackpad and a Linux touchscreen.
- Detection accuracy: the best public figure is about 85% correct (vendor-reported, phone captures). Triage only works if confidence is calibrated; a badly calibrated score destroys trust. Needs a labelled test set early.
- Large touch handles collide on small crops (narrow receipts). Needs hit-priority rules, auto-zoom to crop bounds on entering Adjust, and prototype testing.
- Input conflicts: Windows press-and-hold is a context menu, and edge swipes and 3/4-finger swipes are OS-reserved. Compare-by-long-press must be limited to the canvas and kept off screen edges.
- Perceived speed depends on the engine: loupe and live inset need a tile pyramid and GPU or fast CPU proxy (about 2 MP) so drags hold 60 fps, even for slow-to-decode HEIC or RAW.
- Screen-reader support for a canvas-based crop tool is hard; custom-drawn toolkits depend on AccessKit maturity, and Orca/AT-SPI coverage in webviews is unverified.
- Linux touch is fragmented (X11 vs Wayland, GTK vs Qt); it cannot be fully tested by one maintainer, so label it best-effort at first.
- Scope creep: OCR, multi-item splitting, scanner/camera capture and PDF export each add UI. Decide scope before wireframes are finalised.

## Options evaluated

### Slint (Rust-native UI) — recommended
Declarative Rust-native toolkit; lens here is touch, accessibility and i18n only, and the stack topic should make the final call.
- licence: GPLv3, royalty-free permissive (desktop/mobile/web) or commercial (per slint.dev)
- status: v1.18.1 released 2026-09-21; SwipeGestureHandler since 1.8.0; ScaleRotateGestureHandler since 1.16.0; AccessKit integrated
- pros: Multi-touch pinch/rotate on all platforms plus macOS trackpad gestures; AccessKit gives UIA/NSAccessibility/AT-SPI; No webview; a single Rust codebase and one gesture model
- cons: Quad editor, loupe and ruler must be custom-built; RTL/bidi and complex-script support not evidenced in changelog; verify before committing; Younger widget set than Qt

### Qt Quick / QML (Rust core via bindings) — viable
Mature C++ toolkit with declarative QML; Rust connects through bindings.
- licence: LGPLv3 / GPL / commercial (qt.io)
- status: Qt 6.x; current docs are 6.12; PinchHandler recognises scale, rotation and translation from touchscreen and touchpad
- pros: Most mature multi-touch, accessibility, IME and RTL story; Proven on Windows, macOS and Linux desktops; Rich gesture handlers and Canvas/ShaderEffect for loupe
- cons: Adds a C++/QML toolchain and binding layer to a Rust project; LGPL relinking obligations and larger installers; Native macOS touchpad pinch does not give translation values

### Web UI in Tauri (WebView2 / WKWebView / WebKitGTK) — viable
HTML/CSS/JS front end on the OS webview with a Rust backend.
- licence: Not verified this session (believed permissive); check before relying on it
- status: Actively developed; per-platform engines differ
- pros: Fastest UI iteration; Pointer Events give multi-touch on WebView2; ARIA maps to platform accessibility; mature CSS RTL and i18n; Large design-system ecosystem
- cons: Three engines with different pinch/rotate behaviour; Linux WebKitGTK is the weak leg; Image bytes cross an IPC boundary; loupe and live preview need care; Requires disabling native page zoom per engine

### egui or iced (pure-Rust immediate/Elm-style) — fallback
Pure-Rust GUI libraries with custom-drawn widgets.
- licence: Not verified this session (believed permissive); check before relying on it
- status: egui 0.36.2 (2026-09-08), AccessKit a required dependency since 0.35; iced 0.14 (Dec 2025) with input-method support
- pros: Simple custom canvas drawing for overlays and loupe; egui has active AccessKit and touch-scroll work; Single-language codebase
- cons: Non-native look and feel; gesture recognisers are DIY; Weaker evidence for RTL, IME and screen-reader depth; iced AccessKit not confirmed; Pre-1.0 APIs churn

## Deliverable
**Wireframes (ASCII; epx, wide layout unless noted)**

```
A. IMPORT / FIRST RUN
+--------------------------------------------------------+
| Auto Crop                          [History] [Settings]|
+--------------------------------------------------------+
|  +- - - - - - - - - - - - - - - - - - - - - - - - -+  |
|  :  DROP PHOTOS OR A FOLDER HERE  (tap to browse)  :  |
|  :  JPG PNG HEIC HEIF WebP TIFF RAW ...            :  |
|  +- - - - - - - - - - - - - - - - - - - - - - - - -+  |
|  [Open files] [Open folder] [Paste]      (48 px)      |
|  Preset: (o)Receipts/Docs ( )Photos ( )Convert only   |
|  Recent: Sep-receipts (312) [Resume]   [Try samples]  |
+--------------------------------------------------------+

B. BATCH REVIEW
+--------------------------------------------------------+
|[<] Sep-receipts  (Undo)(Redo)(History)   [Export 312] |
|[Needs review 23][All 312][Edited 4][Skipped 2][Failed 1]|
| Sort: Confidence v        Size -o---+       [Select]  |
| +-------+ +-------+ +-------+ +-------+ +-------+     |
| |       | |       | |       | |       | |       |     |
| |[X]Fail| |[!]Chk | |[!]Chk | |[ok]   | |[ok]   | ... |
| +-------+ +-------+ +-------+ +-------+ +-------+     |
| Thumbs show the CROPPED result. Badge = icon + word.  |
|--------------------------------------------------------|
| 23 need review | 289 look good  [Accept good][Review >]|
+--------------------------------------------------------+

C. EDIT / ADJUST
+---------------------------------------------------------------+
|[< Grid] IMG_07 7/312 [!]Check   (Undo)(Redo) [Compare] [Done]|
+-----+-------------------------------------+-----------------+
|film-|  o-----------o-----------o           | CROP     [Auto] |
|strip|  |   source image + quad  |  (loupe)  | Ratio: Free v   |
| 96  |  o           +           o   3x      | Angle  [ -0.4 ] |
| px  |  |     + centre grip      |  [inset] | [Try option 2/3]|
|     |  o-----------o-----------o  result  | ENHANCE         |
|     |  o = 16 px dot, 48 px hit           | Off|Auto|B&W    |
|     +-------------------------------------+ Strength --o--- |
|     | ruler |..|...0...|..| [-0.4] 90L 90R | Apply: This|Sel|All|
+-----+-------------------------------------+ > Advanced      |
| [Skip X]      [Reset to auto]            [Accept & next  >]  |
+---------------------------------------------------------------+
Portrait/medium: canvas on top; below it ruler, then
[Crop][Rotate][Enhance][More] tab bar, then [Skip] [Accept >].

D. EXPORT (sheet)
+-- Export 312 images -------------------------------+
| Format [JPG v]  Quality [Balanced --o--]  ~41 MB   |
| Resize (o)Original ( )Longest edge [2000] px       |
| Metadata (o)Keep, strip location ( )All ( )None    |
| Colour [x] sRGB  [x] Apply rotation                |
| Save to (o)Next to originals /AutoCrop ( )Choose.. |
|         ( )Replace originals (Advanced)            |
| Name [{name}_crop] -> IMG_0012_crop.jpg            |
| If exists (o)Keep both ( )Skip ( )Replace          |
| > Advanced                                         |
| [Cancel]                          [Export 312]     |
+----------------------------------------------------+
```

**Gesture and input map** (every drag/pinch also has a non-drag path)

| Action | Touch | Trackpad | Mouse | Keyboard |
|---|---|---|---|---|
| Zoom | pinch; +/- buttons | pinch | Ctrl+wheel | + / -, 0 fit, 1 = 100% |
| Pan | drag background or two fingers | two-finger scroll | Space+drag, middle-drag | arrows |
| Fit / 100% | double-tap at point | double-tap | double-click | Z |
| Move corner/edge | 48 px handle, loupe, edge auto-pan | drag | drag | Tab to handle, arrows 1 px, Shift 10 px; numeric fields |
| Move whole quad | centre grip | drag | drag grip | Ctrl+arrows |
| Fine rotate | drag ruler (finger farther from ruler = finer); two-finger twist after 8 degrees | macOS rotate gesture | ruler; Shift = 0.1 degree | Left/Right 0.1, Shift 1, PgUp/PgDn 5, Home 0 |
| Snap / 90 degrees | detent within 1 degree of 0/90/180/270; 90L/90R buttons | same | same | [ ] or R / Shift+R |
| Compare | hold canvas 400 ms or hold Compare | hold Compare | hold Compare, drag split | hold B |
| Next / previous | swipe (Review mode only), chevrons | two-finger swipe | filmstrip click | Left/Right (mirrored in RTL), PgUp/PgDn |
| Accept / skip | thumb-zone buttons | | click | Enter / X |
| Undo / redo | buttons; opt-in 2-/3-finger tap | | buttons | Ctrl/Cmd+Z; Ctrl+Shift+Z or Ctrl+Y |
| Multi-select | long-press, then tap | Cmd/Ctrl-click | Ctrl/Shift-click, rubber-band | Space, Ctrl+A |
| Context menu | press-and-hold thumbnail | two-finger click | right-click | Shift+F10 |
| Cancel drag | drag back to origin | | Esc | Esc |

Other shortcuts (Ctrl on Win/Linux, Cmd on macOS; rebindable; prefer letters over punctuation because Lightroom's backslash compare key fails on some non-US layouts): Open Ctrl+O, Folder Ctrl+Shift+O, Paste Ctrl+V, Export Ctrl+E, Adjust E, Enhance cycle N, Grid overlay G, Reset to auto Ctrl+Shift+A, shortcut sheet ?. Values such as 400 ms, 8 degrees, 48 px and 250 ms are starting points to tune in a prototype.

## Decision-critical claims (as researched)
- Microsoft: 'touchable' is a minimum of 40x40 epx, touch-optimized UI should use 44x44 epx with at least 4 epx between targets; press-and-hold is the standard contextual-menu gesture; apps should not override common gestures and should avoid custom gestures from screen edges; some multi-finger gestures are system-reserved. [https://learn.microsoft.com/en-us/windows/apps/design/input/touch-interactions]
- Apple HIG asks for at least 44x44 pt hit regions for buttons and says not to redefine the standard undo/redo gestures (three-finger swipe left/right, shake); Material recommends 48x48 dp targets with about 8 dp spacing. Apple pages are JS-rendered, so the numbers were confirmed via secondary snippets. [https://developer.apple.com/design/human-interface-guidelines/buttons]
- WCAG 2.2: 2.5.8 Target Size (Minimum, AA) is 24x24 CSS px; 2.5.5 (AAA) is 44x44; 2.5.7 Dragging Movements (AA) requires a non-drag single-pointer alternative (applies to sliders and handles); 2.5.1 Pointer Gestures (A) requires a single-pointer alternative to pinch; 2.5.4 (A) requires a control alternative to shake. [https://www.w3.org/WAI/WCAG22/Understanding/dragging-movements.html]
- Genius Scan's own figures: correct document detection rose from 51% to 75% (2021) to 85% (2024); manual correction was needed for about 50% of documents with the traditional method and about 10% with the hybrid deep-learning method. Vendor-reported on phone captures, so treat as an upper-bound proxy. [https://blog.thegrizzlylabs.com/2024/10/document-detection.html]
- Slint added ScaleRotateGestureHandler in 1.16.0 (2026-04-16) with touchscreen support on all platforms and trackpad pinch/rotate on macOS/iOS; latest release is 1.18.1 (2026-09-21); AccessKit is integrated by Slint, egui, Bevy, Freya and Xilem. RTL/bidi support is not evidenced in the changelog. [https://raw.githubusercontent.com/slint-ui/slint/master/CHANGELOG.md]
- Web-UI gesture parity risk: GestureEvent (Safari trackpad pinch/rotate) is non-standard and WebKit-only, Chromium reports trackpad pinch as ctrl+wheel, and WebKitGTK pinch-zoom is enabled unconditionally (2019 GNOME post, possibly outdated; verify in a spike). WebView2 has an IsPinchZoomEnabled setting. [https://developer.mozilla.org/en-US/docs/Web/API/GestureEvent]
- Windows 11 removed classic tablet mode; posture is derived from keyboard attach/detach (ConvertibleSlateMode), so the UI should adapt to last-used input type rather than an OS mode. Windows 11 reserves left/right edge swipes and three-/four-finger swipes. [https://learn.microsoft.com/en-us/windows-hardware/customize/desktop/settings-for-better-tablet-experiences]
- Comparable-app patterns: Genius Scan offers a magnifier for corner dragging; Snapseed and Lightroom mobile use press-and-hold to show the original; Procreate uses two-finger tap for undo and three-finger tap for redo; ScanTailor Advanced (active fork v1.0.21, 2025-12-20, GPLv3) highlights pages whose skew, content size or margins deviate from the batch. [https://gitea.com/ImageProcessing-ElectronicPublications/scantailor-advanced]

## Researcher questions for user
- What should be the home screen: batch triage, or a single-image editor? — It decides which flow gets the most polish first and how the app opens. (default: Batch-first, with the Receipts/Photos/Convert-only preset chosen at import)
- Should v1 detect and split several receipts or photos in one image (like Photoshop's Crop and Straighten Photos)? — It changes the detection engine, the review UI (per-item chips) and export naming. (default: Single item in v1, with a data model that already supports N quads)
- Which output types beyond images are in scope: PDF, OCR text layer? — Receipts and documents are often wanted as PDFs; this adds export options and settings. (default: Images plus PDF in v1; OCR later)
- Should touch density be fixed or adapt to the input in use? — Affects visual design on desktops with a mouse versus 2-in-1s and tablets. (default: Adapt to last-used pointer; handles keep 44 px hit areas in all modes)
- Where should sessions, edits and undo history persist? — Crash safety on 300-image reviews, privacy and stray files next to originals. (default: Autosave in app data with a capped undo depth)
- How many languages at launch? — Sets translation tooling and whether RTL layout must be built and tested at v1. (default: English plus community translations, with a pseudo-locale and one RTL locale in CI)

## INDEPENDENT VERIFICATION (skeptic) — overrides the researcher where they differ
- [CONFIRMED] 1. Microsoft touch guidance: touchable minimum 40x40 epx; touch-optimized 44x44 epx with at least 4 epx between targets; press-and-hold opens contextual menus; don't override common gestures; avoid custom gestures from screen edges; some multi-finger gestures are system-reserved.
  CORRECTION: All points match the live page (updated 2026-07-14; canonical URL is now /windows/apps/develop/input/touch-interactions). Small precision: the 4 epx is 'visible space' between targets, and 'touchable' also allows 32 epx tall if at least 120 epx wide.
- [CONFIRMED] 2. Apple HIG: at least 44x44 pt hit regions for buttons; don't redefine standard undo/redo gestures (three-finger swipe, shake); Material: 48x48 dp targets with ~8 dp spacing.
  CORRECTION: Confirmed directly from Apple's HIG JSON data (buttons page: at least 44x44 pt, 60x60 in visionOS; undo-and-redo page: avoid redefining standard undo/redo gestures, mentions three-finger swipe and shake; it does not itself spell out left/right). Nuance: the HIG accessibility table gives 44x44 pt default and 28x28 pt minimum on iOS/iPadOS, but macOS default is 28x28 pt (minimum 20x20), so 44 pt is a touch-mode figure, not a Mac desktop one. Material 48dp/8dp confirmed via Google's Android accessibility help.
- [CONFIRMED] 3. WCAG 2.2: 2.5.8 (AA) 24x24 CSS px; 2.5.5 (AAA) 44x44; 2.5.7 Dragging Movements (AA) needs a non-drag single-pointer alternative; 2.5.1 Pointer Gestures (A) needs single-pointer alternative to pinch; 2.5.4 (A) needs control alternative to shake.
  CORRECTION: All levels and sizes verified on the W3C Understanding pages. 2.5.7 explicitly covers sliders and resizing/selection handles; 2.5.1 uses pinch/spread zoom as its example with +/- buttons as the alternative; 2.5.4 uses shake-to-undo as its example and also requires being able to disable motion actuation.
- [PARTLY-TRUE] 4. Genius Scan: correct detection 51% -> 75% (2021) -> 85% (2024); manual correction ~50% with the traditional method and ~10% with the hybrid deep-learning method.
  CORRECTION: I read the blog's raw text. Confirmed: about 50% of documents needed manual readjustment with the traditional approach; correct detection rose 51% -> 75% (neural net plus conventional refinement, 2021); 75% -> 85% in 2024 with a 40% cut in manual adjustments. The '~10% manual with hybrid' figure is NOT in the article and is contradicted by its own numbers (75% correct implies ~25% manual in 2021; 85% implies ~15% in 2024, consistent with a 40% reduction). Also the 51% is the standalone neural network before conventional refinement, not the traditional method. The article does not say how the metric was measured (no dataset or phone-capture statement).
- [CONFIRMED] 5. Slint added ScaleRotateGestureHandler in 1.16.0 (2026-04-16) with touchscreen support on all platforms and trackpad pinch/rotate on macOS/iOS; latest is 1.18.1 (2026-09-21); AccessKit is integrated by Slint, egui, Bevy, Freya and Xilem; RTL/bidi not evidenced in changelog.
  CORRECTION: Versions and dates confirmed from the changelog and GitHub releases (1.18.1 is newest, no later tag). AccessKit adopters list confirmed on accesskit.dev. Precision on platforms: the v1.16.0 docs say macOS/iOS use platform trackpad gesture events, while other platforms process raw two-finger touch input, so Windows/Linux touchpad pinch is not covered by the handler. RTL: changelog has no RTL entry; GitHub issue #2294 (RTL layouts/alignments) is still open, and #7267 reports Persian cursor/word-order bugs in LineEdit. Slint moved text layout to Parley in 1.14.0 (2025-10-21), so shaping may be better than the changelog implies, but layout mirroring is unsupported.
- [PARTLY-TRUE] 6. Web-UI gesture parity risk: GestureEvent is non-standard and WebKit-only; Chromium reports trackpad pinch as ctrl+wheel; WebKitGTK pinch-zoom enabled unconditionally (2019 GNOME post); WebView2 has IsPinchZoomEnabled.
  CORRECTION: Confirmed: MDN marks GestureEvent non-standard and WebKit-specific; WebView2 CoreWebView2Settings.IsPinchZoomEnabled exists (default true, touch pinch only); MDN says zoom actions fire wheel events with ctrlKey true. The 2019 GNOME post itself could not be verified, but the Linux weakness is corroborated by newer evidence: Tauri issue #13115 (Apr 2025, still open) reports pinch scaling the whole page on Ubuntu but not Windows with no API to disable it, pointing to wry #544 (open since 2022), where WebKitGTK handles multi-finger gestures itself. Keep the spike recommendation.
- [PARTLY-TRUE] 7. Windows 11 removed classic tablet mode; posture derived from keyboard attach/detach (ConvertibleSlateMode), so adapt to last-used input type; Windows 11 reserves left/right edge swipes and three-/four-finger swipes.
  CORRECTION: Tablet Mode removal is confirmed, but by Microsoft's Windows 11 specifications page, not the cited page: it says the feature is removed with new keyboard attach/detach posture functionality. The cited page describes posture via SMBIOS Enclosure Type, DeviceForm and ConvertibleSlateMode (OEM-set device-type values, not a live signal), applies only to tablet/convertible/detachable hardware, and Windows stays in desktop mode on laptops/desktops. Edge and multi-finger gestures are confirmed on Microsoft's touch-gesture support page: left edge opens widgets, right edge opens notification center; three-finger swipes show windows, the desktop, or the last app; four-finger left/right switches desktops. Adapting to last-used input is still sound.
- [PARTLY-TRUE] 8. Comparable-app patterns: Genius Scan magnifier for corner dragging; Snapseed and Lightroom mobile press-and-hold to show the original; Procreate two-finger tap undo / three-finger tap redo; ScanTailor Advanced (v1.0.21, 2025-12-20, GPLv3) highlights pages whose skew, content size or margins deviate.
  CORRECTION: Confirmed: Procreate two-finger tap undo and three-finger tap redo (official handbook); ScanTailor Advanced v1.0.21 released 2025-12-20, GPLv3, with red-asterisk deviation flags for skew, content size and margins plus sort-by-deviation. Unverifiable this session (Adobe and Google help pages blocked or 404, no search budget): the Genius Scan corner magnifier and Lightroom mobile press-and-hold. Snapseed's compare gesture is, from memory, a press-and-hold on a compare icon rather than on the image, so treat 'press-and-hold canvas' as a design choice, not a Snapseed precedent. Note ScanTailor Advanced went from v1.0.19 (2023-07) to v1.0.21 (2025-12), so 'active' is modest.

### Other errors spotted by skeptic
- egui option: AccessKit became a required (always-on) dependency in egui 0.34.0 (2026-03-26), not 0.35, per the egui CHANGELOG ('Remove accesskit feature and always depend on accesskit'). egui 0.36.2 (2026-09-08) and iced 0.14 (2025-12-07, input-method support) are correct; iced AccessKit integration is still only an open draft PR (#3111) and open issue #552.
- Qt option: 'current docs are 6.12' is misleading. doc.qt.io shows 6.12, but the Qt release wiki schedules 6.12.0 final for 2026-09-30 (today) and download.qt.io lists 6.11 as the newest released series (6.8 LTS also present). Say latest stable is 6.11, with 6.12 imminent. The PinchHandler caveat (macOS native trackpad gestures give no translation values) is confirmed in the docs.
- Licences marked 'not verified': Tauri is Apache-2.0 OR MIT (GitHub shows Apache-2.0; dual-licensed), egui is MIT OR Apache-2.0, iced is MIT (GitHub API). All permissive, which fits an open-source repo. Tauri stable is 2.12.0 (2026-09-26); a v3.0.0-alpha.3 also exists (2026-09-26), so 'actively developed' holds.
- Slint licence wording: the Royalty-Free License 2.0 is a proprietary licence, not 'permissive' in the OSI sense. It excludes embedded systems, requires attribution (AboutSlint widget or a badge on the download page), and bars distributing an application that exposes Slint APIs. For a free/open-source Auto Crop, the GPLv3 route would push the app to GPL-3.0; the royalty-free route needs the licence-compatibility check before committing.
- Deliverable's 'Left/Right (mirrored in RTL)' shortcut and the RTL/i18n goals conflict with the Slint recommendation: Slint has no RTL layout mirroring yet (issue #2294 open), so this needs an explicit spike item or a Qt fallback.
- Apple 44 pt is the iOS/iPadOS default (minimum 28 pt); macOS default is 28x28 pt (minimum 20x20) and visionOS 60x60, so the '44-48 px everywhere' rule is a deliberate touch-mode choice, not a per-platform Apple requirement.
- Microsoft Lens retirement 'early 2026': only a secondary source (Wikipedia) was found: removed from app stores 2026-02-09, shut down 2026-03-09. A primary Microsoft notice was not located, so the 'local-first gap' framing is plausible but lightly sourced.
- Unverified in the deliverable: the claim that Lightroom's backslash compare shortcut fails on some non-US layouts, and the Genius Scan corner-magnifier precedent. Neither is decision-critical.

## Sources
- https://learn.microsoft.com/en-us/windows/apps/design/input/touch-interactions
- https://learn.microsoft.com/en-us/windows/apps/design/input/guidelines-for-targeting
- https://learn.microsoft.com/en-us/windows/apps/design/layout/screen-sizes-and-breakpoints-for-responsive-design
- https://learn.microsoft.com/en-us/windows-hardware/customize/desktop/settings-for-better-tablet-experiences
- https://learn.microsoft.com/en-us/dotnet/api/microsoft.web.webview2.core.corewebview2settings.ispinchzoomenabled
- https://www.w3.org/WAI/WCAG22/Understanding/target-size-minimum.html
- https://www.w3.org/WAI/WCAG22/Understanding/target-size-enhanced.html
- https://www.w3.org/WAI/WCAG22/Understanding/dragging-movements.html
- https://www.w3.org/WAI/WCAG22/Understanding/pointer-gestures.html
- https://www.w3.org/WAI/WCAG22/Understanding/pointer-cancellation.html
- https://www.w3.org/WAI/WCAG22/Understanding/motion-actuation.html
- https://www.w3.org/WAI/WCAG22/Understanding/non-text-contrast.html
- https://www.w3.org/WAI/WCAG22/Understanding/reflow.html
- https://www.w3.org/WAI/ARIA/apg/patterns/slider/
- https://developer.apple.com/design/human-interface-guidelines/buttons
- https://developer.apple.com/design/human-interface-guidelines/undo-and-redo
- https://developer.apple.com/design/human-interface-guidelines/gestures
- https://support.google.com/accessibility/android/answer/7101858
- https://m3.material.io/foundations/designing/structure
- https://developer.android.com/develop/ui/compose/layouts/adaptive/use-window-size-classes
- https://m2.material.io/go/design-bidirectionality
- https://blog.thegrizzlylabs.com/2024/10/document-detection.html
- https://help.thegrizzlylabs.com/article/152-how-to-crop-and-rotate-a-scan
- https://www.thurrott.com/?p=331644
- https://gitea.com/ImageProcessing-ElectronicPublications/scantailor-advanced
- https://github.com/scantailor/scantailor/wiki/User-Guide/Split-Pages
- https://help.procreate.com/procreate/handbook/interface-gestures/gestures
- https://www.xda-developers.com/how-to-use-touch-gestures-windows-11/
- https://docs.darktable.org/usermanual/stable/en/module-reference/processing-modules/crop/
- https://raw.githubusercontent.com/slint-ui/slint/master/CHANGELOG.md
- https://docs.slint.dev/latest/docs/slint/reference/gestures/scalerotategesturehandler/
- https://accesskit.dev/
- https://slint.dev/
- https://doc.qt.io/qt-6/qml-qtquick-pinchhandler.html
- https://www.qt.io/development/download-open-source
- https://developer.mozilla.org/en-US/docs/Web/API/GestureEvent
- https://blogs.gnome.org/alicem/2019/09/13/gnome-and-gestures-part-1-webkitgtk/
- https://github.com/emilk/egui/releases
- https://github.com/iced-rs/iced/releases
- https://smashingmagazine.com/2016/09/the-thumb-zone-designing-for-mobile-users
- https://developer.apple.com/forums/thread/791283