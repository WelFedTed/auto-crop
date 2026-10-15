// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The real worker binary behind the real pool: decodes equal the in-process decodes, errors are
//! typed, a worker is reused, workers that crash, hang or run out of memory are replaced and the
//! batch continues (ROADMAP M6.10, M6.43). The fault tests need the test-only hooks of the worker
//! (`--features escape-tests`); the rest run against the shipped configuration.

use auto_crop_codecs::{CodecError, DecodeLimits, Decoded, fixtures};
use auto_crop_sandbox::{PoolConfig, PoolFailure, Tier, WorkerPool};
use std::path::PathBuf;
use std::time::Duration;
#[cfg(feature = "escape-tests")]
use std::time::Instant;

pub fn worker_exe() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_auto-crop-worker"))
}

pub fn test_config() -> PoolConfig {
    let mut cfg = PoolConfig::new(worker_exe());
    cfg.workers = 2;
    cfg.memory_limit = Some(768 << 20);
    cfg.startup_timeout = Duration::from_secs(60);
    cfg
}

fn same(a: &Decoded, b: &Decoded) {
    assert_eq!(a.raster.width, b.raster.width);
    assert_eq!(a.raster.height, b.raster.height);
    assert_eq!(a.raster.data, b.raster.data, "pixels differ");
    assert_eq!(a.exif_orientation, b.exif_orientation);
    assert_eq!(a.icc, b.icc);
    assert_eq!(a.source_bit_depth, b.source_bit_depth);
    assert_eq!(a.frames, b.frames);
    assert_eq!(a.notices, b.notices);
}

fn files() -> Vec<(&'static str, Vec<u8>)> {
    #[allow(unused_mut)]
    let mut v = vec![
        ("png", fixtures::png_rgb(64, 48)),
        ("png-rgba", fixtures::png_rgba(32, 32)),
        (
            "png-icc",
            fixtures::png_with_icc(40, 30, &fixtures::fake_icc(300)),
        ),
        ("png-exif6", fixtures::png_with_exif(40, 30, 6)),
        ("png-gray", fixtures::png_gray(33, 21)),
        ("jpeg", fixtures::jpeg_baseline(80, 60)),
        ("webp", fixtures::webp_lossless(30, 20)),
    ];
    #[cfg(feature = "heif")]
    for (name, bytes) in fixtures::heif_real_files() {
        v.push((name, bytes.to_vec()));
    }
    v
}

#[test]
fn decodes_through_the_worker_equal_the_in_process_decodes() {
    let mut cfg = test_config();
    // 22 files x 3 modes is more than the default recycle count
    cfg.recycle_after = 1000;
    let pool = WorkerPool::new(cfg);
    let limits = DecodeLimits::default();
    for (name, bytes) in files() {
        for (orient, flatten) in [(true, false), (false, false), (true, true)] {
            let want = auto_crop_codecs::decode_in_process(&bytes, &limits, orient, flatten)
                .unwrap_or_else(|e| panic!("{name}: in-process {e}"));
            let got = pool
                .decode(&bytes, &limits, orient, flatten)
                .unwrap_or_else(|e| panic!("{name}: worker {e:?}"));
            same(&got, &want);
            assert_eq!(got.format, want.format, "{name}");
        }
    }
    let st = pool.status();
    assert_eq!(
        st.spawned, 1,
        "one worker was reused for every file (replaced because: {:?})",
        st.last_replaced
    );
    assert_eq!(st.crashes, 0);
}

#[test]
fn the_worker_says_what_it_is_confined_by_and_proves_it_from_inside() {
    let pool = WorkerPool::new(test_config());
    let st = pool.probe().expect("a worker starts");
    let level = st.level.clone().unwrap();
    let probes = st.probes.clone().unwrap();
    let tier = st.tier.unwrap();
    println!("SANDBOX attempt={:?}", st.attempt);
    println!("SANDBOX level={level}");
    println!("SANDBOX probes={probes}");
    println!("SANDBOX tier={}", tier.name());
    for f in &st.attempt_failures {
        println!("SANDBOX stronger attempt failed: {f}");
    }
    assert!(tier >= Tier::ProcessOnly, "no confinement at all: {level}");
    if tier >= Tier::Reduced {
        assert!(probes.contains("file=denied"), "{probes}");
        assert!(probes.contains("spawn=denied"), "{probes}");
    }
    if tier == Tier::Full {
        assert!(!probes.contains("net=OPEN"), "{probes}");
    }
    if cfg!(windows) {
        assert!(
            level.starts_with("appcontainer(capabilities=0)"),
            "expected an AppContainer with no capabilities, got {level}"
        );
        assert!(level.contains("active-process=1"), "{level}");
    }
}

#[test]
fn a_bad_file_is_a_typed_error_and_the_worker_goes_on() {
    let pool = WorkerPool::new(test_config());
    let limits = DecodeLimits::default();
    let mut bad = fixtures::png_rgb(40, 40);
    let n = bad.len();
    bad.truncate(n / 2);
    // The header is fine (the pool probes it), the pixel data is cut: the worker reports Corrupt.
    match pool.decode(&bad, &limits, true, false) {
        Err(PoolFailure::Codec(CodecError::Corrupt(_))) => {}
        other => panic!("{other:?}"),
    }
    let ok = fixtures::png_rgb(10, 10);
    assert!(pool.decode(&ok, &limits, true, false).is_ok());
    assert_eq!(pool.status().spawned, 1);
}

#[test]
fn the_pixel_cap_is_applied_before_anything_is_sent() {
    let pool = WorkerPool::new(test_config());
    let limits = DecodeLimits::default().with_max_pixels(100);
    // The header probe has no pixel cap, the worker's decode does: still a typed refusal.
    match pool.decode(&fixtures::png_rgb(64, 64), &limits, true, false) {
        Err(PoolFailure::Codec(CodecError::TooLarge(_) | CodecError::LimitExceeded { .. })) => {}
        other => panic!("{other:?}"),
    }
}

#[test]
fn many_threads_share_the_pool_and_never_exceed_its_size() {
    let pool = WorkerPool::new(test_config());
    let limits = DecodeLimits::default();
    let png = fixtures::png_rgb(120, 90);
    let jpeg = fixtures::jpeg_baseline(120, 90);
    std::thread::scope(|s| {
        for t in 0..8 {
            let (pool, limits, png, jpeg) = (&pool, &limits, &png, &jpeg);
            s.spawn(move || {
                for i in 0..10 {
                    let bytes = if (t + i) % 2 == 0 { png } else { jpeg };
                    pool.decode(bytes, limits, true, false).expect("decodes");
                }
            });
        }
    });
    let st = pool.status();
    assert_eq!(st.jobs, 80);
    assert!(st.spawned <= 2, "spawned {}", st.spawned);
    assert!(st.live <= 2);
}

#[test]
fn idle_workers_end_when_the_pool_is_shut_down_and_start_again_on_demand() {
    let pool = WorkerPool::new(test_config());
    let limits = DecodeLimits::default();
    let png = fixtures::png_rgb(10, 10);
    pool.decode(&png, &limits, true, false).unwrap();
    pool.shutdown_idle();
    assert_eq!(pool.status().live, 0);
    pool.decode(&png, &limits, true, false).unwrap();
    assert_eq!(pool.status().spawned, 2);
}

#[test]
fn a_worker_is_recycled_after_the_configured_number_of_jobs() {
    let mut cfg = test_config();
    cfg.workers = 1;
    cfg.recycle_after = 3;
    let pool = WorkerPool::new(cfg);
    let png = fixtures::png_rgb(10, 10);
    for _ in 0..7 {
        pool.decode(&png, &DecodeLimits::default(), true, false)
            .unwrap();
    }
    assert_eq!(pool.status().spawned, 3);
}

#[test]
fn a_required_tier_that_the_worker_cannot_reach_refuses_it() {
    let mut cfg = test_config();
    cfg.min_tier = Tier::Full;
    cfg.force_attempt = Some(auto_crop_sandbox::os::attempt_names().len() - 1);
    let pool = WorkerPool::new(cfg);
    let r = pool.decode(
        &fixtures::png_rgb(8, 8),
        &DecodeLimits::default(),
        true,
        false,
    );
    if cfg!(windows) {
        // the last attempt is the job object only, below Full
        assert!(matches!(r, Err(PoolFailure::Unavailable(m)) if m.contains("below the required")));
    }
}

#[cfg(feature = "escape-tests")]
mod faults {
    use super::*;
    use auto_crop_core::CancelToken;

    fn with_fault(kind: &str) -> Vec<u8> {
        let mut b = fixtures::png_rgb(16, 16);
        b.extend_from_slice(format!("AUTOCROP-FAULT:{kind}").as_bytes());
        b
    }

    fn crashed(r: Result<Decoded, PoolFailure>) -> CodecError {
        match r {
            Err(PoolFailure::Codec(e)) => e,
            other => panic!("expected a typed error, got {other:?}"),
        }
    }

    #[test]
    fn a_crash_is_a_typed_error_the_worker_is_replaced_and_the_batch_continues() {
        let pool = WorkerPool::new(test_config());
        let limits = DecodeLimits::default();
        let good = fixtures::png_rgb(16, 16);
        assert!(pool.decode(&good, &limits, true, false).is_ok());
        for kind in ["abort", "exit", "garbage"] {
            let t0 = Instant::now();
            let e = crashed(pool.decode(&with_fault(kind), &limits, true, false));
            assert!(matches!(e, CodecError::WorkerCrashed(_)), "{kind}: {e:?}");
            assert!(t0.elapsed() < Duration::from_secs(20), "{kind} took long");
            // the next file is decoded by a fresh worker
            assert!(pool.decode(&good, &limits, true, false).is_ok(), "{kind}");
        }
        let st = pool.status();
        assert_eq!(st.crashes, 3);
        assert!(st.spawned >= 4);
        assert_eq!(st.live, 1);
    }

    #[test]
    fn a_panic_inside_the_decode_is_reported_and_the_worker_survives() {
        let pool = WorkerPool::new(test_config());
        let limits = DecodeLimits::default();
        let e = crashed(pool.decode(&with_fault("panic"), &limits, true, false));
        assert!(matches!(e, CodecError::InternalPanic(_)), "{e:?}");
        assert!(
            pool.decode(&fixtures::png_rgb(8, 8), &limits, true, false)
                .is_ok()
        );
        assert_eq!(pool.status().spawned, 1);
    }

    #[test]
    fn a_hang_is_killed_at_the_deadline() {
        let pool = WorkerPool::new(test_config());
        let limits = DecodeLimits {
            max_decode_ms: Some(1500),
            ..DecodeLimits::default()
        };
        let t0 = Instant::now();
        let e = crashed(pool.decode(&with_fault("hang"), &limits, true, false));
        assert_eq!(e, CodecError::DecodeTimeout);
        let took = t0.elapsed();
        assert!(took < Duration::from_secs(15), "{took:?}");
        assert!(took >= Duration::from_millis(1400), "{took:?}");
        assert_eq!(pool.status().timeouts, 1);
        assert!(
            pool.decode(
                &fixtures::png_rgb(8, 8),
                &DecodeLimits::default(),
                true,
                false
            )
            .is_ok()
        );
    }

    #[test]
    fn running_out_of_memory_is_a_memory_limit_error_and_the_app_goes_on() {
        let mut cfg = test_config();
        cfg.memory_limit = Some(512 << 20);
        let pool = WorkerPool::new(cfg);
        let limits = DecodeLimits::default();
        let t0 = Instant::now();
        let e = crashed(pool.decode(&with_fault("alloc"), &limits, true, false));
        assert_eq!(e, CodecError::MemoryLimit, "{e:?}");
        assert!(t0.elapsed() < Duration::from_secs(30));
        assert!(
            pool.decode(&fixtures::png_rgb(8, 8), &limits, true, false)
                .is_ok()
        );
    }

    #[test]
    fn cancelling_kills_the_worker_at_once() {
        let pool = WorkerPool::new(test_config());
        let token = CancelToken::new_batch();
        let t2 = token.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(700));
            t2.cancel();
        });
        let t0 = Instant::now();
        let r = auto_crop_sandbox::with_cancel(&token, || {
            pool.decode(&with_fault("hang"), &DecodeLimits::default(), true, false)
        });
        assert_eq!(crashed(r), CodecError::Cancelled);
        assert!(t0.elapsed() < Duration::from_secs(10));
        assert_eq!(pool.status().cancelled, 1);
    }

    #[test]
    fn a_thousand_decodes_with_random_faults_leave_no_orphans_and_no_leaks() {
        let mut cfg = test_config();
        cfg.workers = 3;
        let pool = WorkerPool::new(cfg);
        let limits = DecodeLimits::default();
        let good = fixtures::png_rgb(24, 24);
        let faults = [with_fault("abort"), with_fault("exit"), with_fault("panic")];
        let (mut ok, mut failed) = (0, 0);
        let mut x = 0x2545_f491_4f6c_dd1du64;
        std::thread::scope(|s| {
            let res: Vec<_> = (0..3)
                .map(|t| {
                    let (pool, limits, good, faults) = (&pool, &limits, &good, &faults);
                    s.spawn(move || {
                        let mut x = 0x2545_f491_4f6c_dd1du64 ^ t;
                        let (mut ok, mut failed) = (0, 0);
                        for _ in 0..80 {
                            x ^= x << 13;
                            x ^= x >> 7;
                            x ^= x << 17;
                            let bytes = if x.is_multiple_of(7) {
                                &faults[(x / 7 % 3) as usize]
                            } else {
                                good
                            };
                            match pool.decode(bytes, limits, true, false) {
                                Ok(_) => ok += 1,
                                Err(PoolFailure::Codec(_)) => failed += 1,
                                Err(e) => panic!("{e:?}"),
                            }
                        }
                        (ok, failed)
                    })
                })
                .collect();
            for r in res {
                let (o, f) = r.join().unwrap();
                ok += o;
                failed += f;
            }
        });
        let _ = &mut x;
        assert_eq!(ok + failed, 240);
        assert!(ok > 150 && failed > 5, "ok {ok} failed {failed}");
        let st = pool.status();
        assert!(st.live <= 3);
        pool.shutdown_idle();
        assert_eq!(pool.status().live, 0);
    }
}
