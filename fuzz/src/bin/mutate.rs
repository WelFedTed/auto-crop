// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `mutate [rounds] [seed] [target]`: a tiny dependency-free mutation fuzzer over the seed corpus, for
//! machines where libFuzzer cannot run (Windows, macOS). It is blind (no coverage feedback), so it is
//! no substitute for the CI fuzz jobs, but it finds shallow invariant failures in seconds. A failing
//! input is written to `fuzz/artifacts-local/<target>/` and printed; copy it to `fuzz/regressions/`
//! once fixed.
//!
//!   cargo run --release --manifest-path fuzz/Cargo.toml --no-default-features --bin mutate -- 200

use auto_crop_fuzz::{TARGETS, replay, seeds};

static LAST_PANIC: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

const INTERESTING: &[&[u8]] = &[
    &[0],
    &[0xFF],
    &[0x7F],
    &[0x80],
    &[0xFF, 0xFF],
    &[0, 0, 0, 0],
    &[0xFF, 0xFF, 0xFF, 0xFF],
    &[0x7F, 0xFF, 0xFF, 0xFF],
    &[0, 0, 0, 1],
    &[1, 0, 0, 0],
    &[0xFF, 0xD9],
    &[0xFF, 0xDA],
    &[0xFF, 0xC2],
    b"IDAT",
    b"iCCP",
    b"eXIf",
    b"ICC_PROFILE\0",
];

fn mutate(rng: &mut Rng, base: &[u8], other: &[u8]) -> Vec<u8> {
    let mut v = base.to_vec();
    for _ in 0..=rng.below(4) {
        match rng.below(8) {
            0 if !v.is_empty() => {
                let i = rng.below(v.len());
                v[i] ^= 1 << rng.below(8);
            }
            1 if !v.is_empty() => {
                let i = rng.below(v.len());
                v[i] = rng.next() as u8;
            }
            2 if !v.is_empty() => {
                let i = rng.below(v.len());
                let n = rng.below(16).min(v.len() - i);
                v.drain(i..i + n);
            }
            3 => {
                let i = rng.below(v.len() + 1);
                let n = 1 + rng.below(8);
                let ins: Vec<u8> = (0..n).map(|_| rng.next() as u8).collect();
                v.splice(i..i, ins);
            }
            4 if !v.is_empty() => {
                let i = rng.below(v.len());
                let k = INTERESTING[rng.below(INTERESTING.len())];
                let n = k.len().min(v.len() - i);
                v[i..i + n].copy_from_slice(&k[..n]);
            }
            5 => {
                let n = rng.below(v.len() + 1);
                v.truncate(n);
            }
            6 if !other.is_empty() => {
                // Splice a piece of another seed in.
                let a = rng.below(other.len());
                let b = a + rng.below((other.len() - a).min(64) + 1);
                let i = rng.below(v.len() + 1);
                v.splice(i..i, other[a..b].iter().copied());
            }
            7 if !v.is_empty() => {
                // Little-endian and big-endian +-1 on a 32-bit field.
                let i = rng.below(v.len().saturating_sub(3).max(1));
                if let Some(w) = v.get_mut(i..i + 4) {
                    let x = u32::from_be_bytes([w[0], w[1], w[2], w[3]]);
                    let y = if rng.below(2) == 0 {
                        x.wrapping_add(1)
                    } else {
                        x.wrapping_sub(1)
                    };
                    w.copy_from_slice(&y.to_be_bytes());
                }
            }
            _ => {}
        }
    }
    v.truncate(auto_crop_fuzz::MAX_INPUT);
    v
}

fn main() {
    let mut args = std::env::args().skip(1);
    let rounds: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(50);
    let seed: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(1);
    let only = args.next();
    // Panics are the findings: remember where the last one happened instead of printing every one.
    std::panic::set_hook(Box::new(|info| {
        let at = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_default();
        let mut last = LAST_PANIC.lock().unwrap();
        // Keep the first panic of an execution: the guard re-raises the original one.
        if last.is_empty() {
            *last = format!("at {at}");
        }
    }));

    let all = seeds::seeds();
    let mut failures = 0usize;
    let mut execs = 0usize;
    for (target, f) in TARGETS {
        if only.as_deref().is_some_and(|o| o != *target) {
            continue;
        }
        let pool: Vec<&[u8]> = all
            .iter()
            .filter(|s| s.target == *target)
            .map(|s| s.bytes.as_slice())
            .collect();
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let t = std::time::Instant::now();
        for round in 0..rounds {
            for base in &pool {
                let other = pool[rng.below(pool.len())];
                let input = mutate(&mut rng, base, other);
                execs += 1;
                LAST_PANIC.lock().unwrap().clear();
                if let Err(e) = replay(*f, &input) {
                    failures += 1;
                    let dir = std::path::Path::new("fuzz/artifacts-local").join(target);
                    let _ = std::fs::create_dir_all(&dir);
                    let name = format!("fail-r{round}-{failures}");
                    let _ = std::fs::write(dir.join(&name), &input);
                    println!(
                        "FAIL {target} ({} bytes) saved as {name}: {e} [{}]",
                        input.len(),
                        LAST_PANIC.lock().unwrap()
                    );
                }
            }
        }
        println!(
            "{target}: {} seeds x {rounds} rounds in {:?}",
            pool.len(),
            t.elapsed()
        );
    }
    println!("{execs} executions, {failures} failures");
    std::process::exit(i32::from(failures > 0));
}
