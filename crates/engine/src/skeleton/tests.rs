// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

use super::*;
use auto_crop_imgproc::synth::{PaperKind, Scene, render_scene};
use std::sync::Mutex;
use tracing_subscriber::layer::SubscriberExt;

/// A deterministic JPEG of a paper page on a desk.
fn photo(w: u32, h: u32) -> Vec<u8> {
    let scene = Scene {
        width: w,
        height: h,
        background: [120, 100, 80],
        paper: [244, 242, 236],
        ink: [60, 64, 76],
        kind: PaperKind::Document,
        corners: [(0.18, 0.10), (0.82, 0.14), (0.80, 0.92), (0.20, 0.88)],
        seed: 11,
        noise: 3.0,
        blur_radius: 1,
        shadow: true,
    };
    encode(&render_scene(&scene), Format::Jpeg, 90, None).unwrap()
}

/// Records span names (creation order) and `stage_done` events.
#[derive(Default, Clone)]
struct Capture {
    spans: Arc<Mutex<Vec<String>>>,
    events: Arc<Mutex<Vec<(String, f64, u64)>>>,
}

#[derive(Default)]
struct Fields {
    message: String,
    stage: String,
    ms: f64,
    px: u64,
}

impl tracing::field::Visit for Fields {
    fn record_f64(&mut self, f: &tracing::field::Field, v: f64) {
        if f.name() == "ms" {
            self.ms = v;
        }
    }
    fn record_u64(&mut self, f: &tracing::field::Field, v: u64) {
        if f.name() == "px" {
            self.px = v;
        }
    }
    fn record_str(&mut self, f: &tracing::field::Field, v: &str) {
        if f.name() == "stage" {
            self.stage = v.to_owned();
        }
    }
    fn record_debug(&mut self, f: &tracing::field::Field, v: &dyn std::fmt::Debug) {
        if f.name() == "message" {
            self.message = format!("{v:?}");
        }
    }
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Capture {
    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        _: &tracing::span::Id,
        _: tracing_subscriber::layer::Context<'_, S>,
    ) {
        self.spans
            .lock()
            .unwrap()
            .push(attrs.metadata().name().to_owned());
    }

    fn on_event(&self, e: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        let mut f = Fields::default();
        e.record(&mut f);
        if f.message == "stage_done" {
            self.events.lock().unwrap().push((f.stage, f.ms, f.px));
        }
    }
}

fn captured<T>(f: impl FnOnce() -> T) -> (T, Capture) {
    // `tracing` caches, per call site, whether any subscriber is interested. Tests run in
    // parallel threads, and a call site first hit on a thread with no subscriber is cached as
    // "never" and would drop this thread's scoped events. A global default (any one; the first
    // set wins) that is interested in everything keeps every call site "sometimes", so the
    // scoped dispatcher below always sees them.
    static GLOBAL: std::sync::Once = std::sync::Once::new();
    GLOBAL.call_once(|| {
        let _ = tracing::subscriber::set_global_default(tracing_subscriber::registry());
    });
    let cap = Capture::default();
    let sub = tracing_subscriber::registry().with(cap.clone());
    let out = tracing::subscriber::with_default(sub, f);
    (out, cap)
}

fn never() -> CancelToken {
    CancelToken::never()
}

#[test]
fn every_stage_reports_in_table_a_order_with_matching_spans_and_events() {
    let jpeg = photo(1600, 1200);
    let (out, cap) = captured(|| {
        run(Input::Bytes(&jpeg), &Options::default(), &never()).expect("pipeline runs")
    });
    let names: Vec<&str> = Stage::ALL.iter().map(|s| s.name()).collect();
    assert_eq!(
        names,
        [
            "read_probe",
            "decode",
            "proxy",
            "analyse",
            "rectify",
            "enhance",
            "encode"
        ]
    );
    let reported: Vec<&str> = out.report.stages.iter().map(|s| s.stage.name()).collect();
    assert_eq!(reported, names, "every stage reports, in order");
    assert_eq!(*cap.spans.lock().unwrap(), names, "span order");
    let events = cap.events.lock().unwrap();
    assert_eq!(events.len(), 7);
    for (ev, st) in events.iter().zip(&out.report.stages) {
        assert_eq!(ev.0, st.stage.name());
        assert_eq!(ev.1, st.ms);
        assert_eq!(ev.2, st.px);
        assert!(st.px > 0 && st.ms >= 0.0);
    }
    // Decoded size is what the probe saw; rectify, enhance and encode work on the output.
    let px = |s: Stage| out.report.stages.iter().find(|t| t.stage == s).unwrap().px;
    assert_eq!(px(Stage::ReadProbe), 1600 * 1200);
    assert_eq!(px(Stage::Decode), 1600 * 1200);
    assert_eq!(px(Stage::Proxy), 1600 * 1200);
    assert!(px(Stage::Analyse) <= 1024 * 1024);
    let out_px = u64::from(out.report.output.0) * u64::from(out.report.output.1);
    assert_eq!(px(Stage::Rectify), out_px);
    assert_eq!(px(Stage::Enhance), out_px);
    assert_eq!(px(Stage::Encode), out_px);
    assert!(out.report.quad_found, "the page is found");
    assert!(out_px < 1600 * 1200, "the desk is cropped away");
    // The result is a real JPEG of the reported size.
    let back = auto_crop_codecs::decode(&out.bytes).unwrap();
    assert_eq!((back.raster.width, back.raster.height), out.report.output);
    assert_eq!(out.report.output_bytes, out.bytes.len());
}

#[test]
fn the_stage_spans_sum_to_the_total_within_the_glue() {
    let jpeg = photo(2400, 1800);
    // Warm up once so one-off costs (page faults, lazy statics) do not count against the glue.
    run(Input::Bytes(&jpeg), &Options::default(), &never()).unwrap();
    for _ in 0..3 {
        let r = run(Input::Bytes(&jpeg), &Options::default(), &never())
            .unwrap()
            .report;
        let sum = r.stage_sum_ms();
        assert!(sum <= r.total_ms + 1e-6, "sum {sum} > total {}", r.total_ms);
        // The glue between stages is dropping buffers and moving values: a few percent at most.
        let glue = r.total_ms - sum;
        assert!(
            glue <= (0.15 * r.total_ms).max(25.0),
            "glue {glue:.2} ms of {:.2} ms total",
            r.total_ms
        );
    }
}

#[test]
fn a_file_input_includes_the_read_and_matches_the_bytes_input() {
    let jpeg = photo(1200, 900);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("p.jpg");
    std::fs::write(&path, &jpeg).unwrap();
    let a = run(Input::Path(&path), &Options::default(), &never()).unwrap();
    let b = run(Input::Bytes(&jpeg), &Options::default(), &never()).unwrap();
    assert_eq!(a.bytes, b.bytes);
}

#[test]
fn the_output_bytes_are_identical_at_1_and_8_threads() {
    let jpeg = photo(2000, 1500);
    for enhance in [Enhance::Otsu, Enhance::Sauvola, Enhance::Off] {
        let with = |threads: usize| {
            let opts = Options {
                enhance,
                pool: Some(thread_pool(threads).unwrap()),
                ..Options::default()
            };
            run(Input::Bytes(&jpeg), &opts, &never()).unwrap().bytes
        };
        let (one, eight) = (with(1), with(8));
        assert_eq!(one, eight, "{enhance:?}: 1 and 8 threads differ");
        assert_eq!(one, with(3), "{enhance:?}: 3 threads differ");
    }
}

#[test]
fn events_reach_the_callers_subscriber_from_inside_a_pool() {
    let jpeg = photo(800, 600);
    let (_, cap) = captured(|| {
        let opts = Options {
            pool: Some(thread_pool(2).unwrap()),
            ..Options::default()
        };
        run(Input::Bytes(&jpeg), &opts, &never()).unwrap()
    });
    assert_eq!(cap.events.lock().unwrap().len(), 7);
    assert_eq!(cap.spans.lock().unwrap().len(), 7);
}

#[test]
fn crop_only_skips_enhance_and_still_reports_the_rest() {
    let jpeg = photo(1200, 900);
    let opts = Options {
        enhance: Enhance::Off,
        ..Options::default()
    };
    let (out, cap) = captured(|| run(Input::Bytes(&jpeg), &opts, &never()).unwrap());
    assert_eq!(out.report.stages.len(), 6);
    assert!(out.report.ms_of(Stage::Enhance).is_none());
    assert!(!cap.spans.lock().unwrap().iter().any(|s| s == "enhance"));
}

#[test]
fn enhance_makes_the_page_black_and_white() {
    let jpeg = photo(1200, 900);
    let on = run(Input::Bytes(&jpeg), &Options::default(), &never()).unwrap();
    let back = auto_crop_codecs::decode(&on.bytes).unwrap().raster;
    // After JPEG, almost every pixel is still close to black or white.
    let extreme = back
        .data
        .as_chunks::<3>()
        .0
        .iter()
        .filter(|p| p.iter().all(|c| *c < 40 || *c > 215))
        .count();
    assert!(extreme * 10 > back.data.len() / 3 * 9, "{extreme} extreme");
}

#[test]
fn a_token_cancelled_before_the_start_stops_at_the_first_stage() {
    let jpeg = photo(400, 300);
    let token = CancelToken::new_batch();
    token.cancel();
    let f = run(Input::Bytes(&jpeg), &Options::default(), &token).unwrap_err();
    assert_eq!(f.kind, ErrKind::Cancelled);
    assert_eq!(f.stage, Stage::ReadProbe);
    assert!(f.done.is_empty());
}

#[test]
fn cancelling_after_any_stage_stops_at_the_next_one_with_no_output() {
    let jpeg = photo(800, 600);
    for (i, after) in Stage::ALL.iter().take(6).enumerate() {
        let token = CancelToken::new_batch();
        let t2 = token.clone();
        let mut seen = Vec::new();
        let r = run_observed(Input::Bytes(&jpeg), &Options::default(), &token, &mut |s| {
            seen.push(s.stage);
            if s.stage == *after {
                t2.cancel();
            }
        });
        let f = r.expect_err("a cancelled run returns no output");
        assert_eq!(f.kind, ErrKind::Cancelled, "after {after:?}");
        assert_eq!(f.stage, Stage::ALL[i + 1], "after {after:?}");
        assert_eq!(f.done.len(), i + 1);
        assert_eq!(seen.len(), i + 1);
    }
}

#[test]
fn a_passed_deadline_is_reported_as_such() {
    let jpeg = photo(400, 300);
    let token = CancelToken::new_batch().child_with_timeout(std::time::Duration::ZERO);
    let f = run(Input::Bytes(&jpeg), &Options::default(), &token).unwrap_err();
    assert_eq!(f.kind, ErrKind::DeadlineExceeded);
}

#[test]
fn garbage_is_an_unsupported_format_at_read_probe() {
    let f = run(
        Input::Bytes(b"this is not an image at all"),
        &Options::default(),
        &never(),
    )
    .unwrap_err();
    assert_eq!(
        (f.kind, f.stage),
        (ErrKind::UnsupportedFormat, Stage::ReadProbe)
    );
    assert!(f.done.is_empty());
}

#[test]
fn a_missing_file_is_unreadable_at_read_probe() {
    let dir = tempfile::tempdir().unwrap();
    let f = run(
        Input::Path(&dir.path().join("nope.jpg")),
        &Options::default(),
        &never(),
    )
    .unwrap_err();
    assert_eq!((f.kind, f.stage), (ErrKind::Unreadable, Stage::ReadProbe));
}

#[test]
fn a_truncated_jpeg_fails_with_a_code_and_the_stages_before_it() {
    let jpeg = photo(800, 600);
    let cut = &jpeg[..jpeg.len() / 3];
    let f = run(Input::Bytes(cut), &Options::default(), &never()).unwrap_err();
    assert!(
        matches!(f.kind, ErrKind::Corrupt | ErrKind::UnsupportedFormat),
        "{:?}",
        f.kind
    );
    assert!(matches!(f.stage, Stage::ReadProbe | Stage::Decode));
    assert_eq!(f.done.len(), usize::from(f.stage == Stage::Decode));
}

#[test]
fn the_pixel_cap_stops_the_decode_stage() {
    let jpeg = photo(800, 600);
    let opts = Options {
        limits: DecodeLimits::default().with_max_pixels(100_000),
        ..Options::default()
    };
    let f = run(Input::Bytes(&jpeg), &opts, &never()).unwrap_err();
    assert_eq!(f.kind, ErrKind::TooLarge);
    assert_eq!(f.stage, Stage::Decode);
    assert_eq!(f.done.len(), 1, "read_probe finished");
}

#[test]
fn a_failed_stage_emits_no_stage_done() {
    let (r, cap) = captured(|| run(Input::Bytes(b"xxxx"), &Options::default(), &never()));
    assert!(r.is_err());
    assert!(cap.events.lock().unwrap().is_empty());
    assert_eq!(*cap.spans.lock().unwrap(), ["read_probe"]);
}

#[test]
fn the_chained_budget_is_the_table_a_lines_that_run() {
    assert_eq!(chained_budget_ms(), 515.0);
}

#[test]
fn the_timings_table_lists_every_stage_and_labels_the_stand_ins() {
    let jpeg = photo(800, 600);
    let r = run(Input::Bytes(&jpeg), &Options::default(), &never())
        .unwrap()
        .report;
    let t = format_timings(&r);
    for s in Stage::ALL {
        assert!(t.contains(s.name()), "{t}");
    }
    assert!(t.contains("STAND-IN") && t.contains("PROTOTYPE") && t.contains("PROVISIONAL"));
    assert!(t.contains("800x600"));
}
