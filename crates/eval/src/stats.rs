// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Small, dependency-free statistics: quantiles, the one-sided Clopper-Pearson upper bound used for
//! the silent-failure gate (PLAN 7.4), Wilson intervals for the reliability diagram and a seeded
//! bootstrap for the mean IoU (ROADMAP M1.47).

/// Slices with fewer images than this are never reported publicly (golden-set policy).
pub const MIN_PUBLIC_N: usize = 30;
/// Slices with fewer images than this are advisory: shown, never gated (PLAN 7.4).
pub const MIN_GATE_N: usize = 80;

/// Quantile `q` in 0..=1 of a **sorted** slice by linear interpolation between order statistics
/// (NumPy's default, "type 7"). `NaN` for an empty slice.
pub fn quantile_sorted(sorted: &[f64], q: f64) -> f64 {
    match sorted.len() {
        0 => f64::NAN,
        1 => sorted[0],
        n => {
            let pos = q.clamp(0.0, 1.0) * (n - 1) as f64;
            let lo = pos.floor() as usize;
            let hi = (lo + 1).min(n - 1);
            let frac = pos - lo as f64;
            sorted[lo] + (sorted[hi] - sorted[lo]) * frac
        }
    }
}

/// Quantile of unsorted data.
pub fn quantile(values: &[f64], q: f64) -> f64 {
    let mut v = values.to_vec();
    v.sort_by(f64::total_cmp);
    quantile_sorted(&v, q)
}

pub fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        return f64::NAN;
    }
    values.iter().sum::<f64>() / values.len() as f64
}

/// `P(X <= k)` for `X ~ Binomial(n, p)`, summed in log space.
pub fn binom_cdf(k: u64, n: u64, p: f64) -> f64 {
    if k >= n {
        return 1.0;
    }
    if p <= 0.0 {
        return 1.0;
    }
    if p >= 1.0 {
        return 0.0;
    }
    let (lp, lq) = (p.ln(), (1.0 - p).ln());
    let mut ln_choose = 0.0f64; // ln C(n, 0)
    let mut terms = Vec::with_capacity(k as usize + 1);
    for i in 0..=k {
        terms.push(ln_choose + i as f64 * lp + (n - i) as f64 * lq);
        ln_choose += ((n - i) as f64).ln() - ((i + 1) as f64).ln();
    }
    let max = terms.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let s: f64 = terms.iter().map(|t| (t - max).exp()).sum();
    (max + s.ln()).exp().min(1.0)
}

/// The one-sided Clopper-Pearson upper confidence bound on a rate after `k` events in `n` trials:
/// the `p` with `P(X <= k | n, p) = alpha`. `1.0` when `k >= n`; `NaN` when `n == 0`.
pub fn clopper_pearson_upper(k: u64, n: u64, alpha: f64) -> f64 {
    if n == 0 {
        return f64::NAN;
    }
    if k >= n {
        return 1.0;
    }
    // binom_cdf decreases from 1 (p = 0) to 0 (p = 1) in p; bisect for the crossing.
    let (mut lo, mut hi) = (0.0f64, 1.0f64);
    for _ in 0..200 {
        let mid = 0.5 * (lo + hi);
        if binom_cdf(k, n, mid) > alpha {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    0.5 * (lo + hi)
}

/// The Wilson score interval for `k` successes in `n` trials at 95% (z = 1.96).
pub fn wilson_95(k: u64, n: u64) -> (f64, f64) {
    if n == 0 {
        return (f64::NAN, f64::NAN);
    }
    let z = 1.959_963_984_540_054f64;
    let (nf, p) = (n as f64, k as f64 / n as f64);
    let denom = 1.0 + z * z / nf;
    let centre = (p + z * z / (2.0 * nf)) / denom;
    let half = z * (p * (1.0 - p) / nf + z * z / (4.0 * nf * nf)).sqrt() / denom;
    ((centre - half).max(0.0), (centre + half).min(1.0))
}

/// SplitMix64: tiny, fast, good statistical quality, identical on every platform.
pub struct SplitMix64(pub u64);

impl SplitMix64 {
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n` (multiply-shift; the bias is below 2^-64 * n).
    pub fn below(&mut self, n: usize) -> usize {
        ((u128::from(self.next_u64()) * n as u128) >> 64) as usize
    }

    /// Uniform in `[0, 1)`.
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// Resamples used for every bootstrap interval; fixed so reports are reproducible.
pub const BOOTSTRAP_RESAMPLES: usize = 2000;
/// The fixed bootstrap seed.
pub const BOOTSTRAP_SEED: u64 = 0xA070_C20B_5EED_0001;

/// Two-sided percentile bootstrap interval for the mean at the given `level` (0.95 -> 2.5% and 97.5%).
/// Deterministic for a given `seed`. `None` for an empty input.
pub fn bootstrap_mean_ci(
    values: &[f64],
    resamples: usize,
    seed: u64,
    level: f64,
) -> Option<(f64, f64)> {
    let n = values.len();
    if n == 0 {
        return None;
    }
    let mut rng = SplitMix64(seed);
    let mut means = Vec::with_capacity(resamples);
    for _ in 0..resamples {
        let mut s = 0.0;
        for _ in 0..n {
            s += values[rng.below(n)];
        }
        means.push(s / n as f64);
    }
    means.sort_by(f64::total_cmp);
    let tail = (1.0 - level) / 2.0;
    Some((
        quantile_sorted(&means, tail),
        quantile_sorted(&means, 1.0 - tail),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantiles_match_numpy_linear() {
        // numpy.percentile([1, 2, 3, 4, 10], [0, 25, 50, 75, 90, 100]) = 1, 2, 3, 4, 7.6, 10
        let v = [10.0, 1.0, 3.0, 2.0, 4.0];
        for (q, want) in [
            (0.0, 1.0),
            (0.25, 2.0),
            (0.5, 3.0),
            (0.75, 4.0),
            (0.9, 7.6),
            (1.0, 10.0),
        ] {
            assert!((quantile(&v, q) - want).abs() < 1e-12, "{q}");
        }
        // numpy.percentile([0.1, 0.4], 50) = 0.25
        assert!((quantile(&[0.1, 0.4], 0.5) - 0.25).abs() < 1e-15);
        assert_eq!(quantile(&[7.0], 0.99), 7.0);
        assert!(quantile(&[], 0.5).is_nan());
    }

    #[test]
    fn zero_event_bound_has_a_closed_form() {
        // k = 0: the bound solves (1 - p)^n = alpha, so p = 1 - alpha^(1/n).
        for n in [1u64, 10, 149, 200, 741, 5000] {
            let want = 1.0 - 0.05f64.powf(1.0 / n as f64);
            let got = clopper_pearson_upper(0, n, 0.05);
            assert!((got - want).abs() < 1e-12, "n={n}: {got} vs {want}");
        }
        // PLAN 7.4: zero failures reach 2.0% only from 149 accepted images.
        assert!(clopper_pearson_upper(0, 148, 0.05) > 0.02);
        assert!(clopper_pearson_upper(0, 149, 0.05) <= 0.02);
    }

    #[test]
    fn upper_bound_satisfies_its_defining_equation() {
        for (k, n) in [
            (1u64, 20u64),
            (3, 100),
            (6, 741),
            (7, 741),
            (4, 450),
            (50, 1000),
        ] {
            let ub = clopper_pearson_upper(k, n, 0.05);
            assert!((binom_cdf(k, n, ub) - 0.05).abs() < 1e-9, "k={k} n={n}");
        }
        // The Beta quantile form gives the same number for k = 1 (closed form for n = 2):
        // P(X <= 1 | 2, p) = 1 - p^2 = 0.05 -> p = sqrt(0.95).
        assert!((clopper_pearson_upper(1, 2, 0.05) - 0.95f64.sqrt()).abs() < 1e-12);
        assert_eq!(clopper_pearson_upper(5, 5, 0.05), 1.0);
        assert!(clopper_pearson_upper(0, 0, 0.05).is_nan());
    }

    #[test]
    fn plan_7_4_worked_examples() {
        // "6 failures among 741 auto-accepted images is 0.81% with a bound of ~1.6%".
        let ub6 = clopper_pearson_upper(6, 741, 0.05);
        assert!((ub6 - 0.0159).abs() < 5e-4, "{ub6}");
        // "at ~740 accepted images tolerates at most 7 failures (bound ~1.8%)".
        let ub7 = clopper_pearson_upper(7, 741, 0.05);
        assert!(ub7 <= 0.02 && (ub7 - 0.0180).abs() < 5e-4, "{ub7}");
        // The 8th failure breaks the point-estimate gate (8/741 = 1.08% > 1.0%) even though the
        // bound (about 1.95%) is still under 2.0%: the point estimate is what binds at n = 741.
        let ub8 = clopper_pearson_upper(8, 741, 0.05);
        assert!(8.0 / 741.0 > 0.01 && ub8 <= 0.02 && ub8 > ub7, "{ub8}");
        // "at ~450 it tolerates at most 3 (4 failures give 2.02%)".
        assert!(clopper_pearson_upper(3, 450, 0.05) <= 0.02);
        let ub4 = clopper_pearson_upper(4, 450, 0.05);
        assert!((ub4 - 0.0202).abs() < 5e-4, "{ub4}");
    }

    #[test]
    fn binomial_cdf_matches_hand_values() {
        // Binomial(10, 0.5): P(X <= 3) = 176/1024.
        assert!((binom_cdf(3, 10, 0.5) - 176.0 / 1024.0).abs() < 1e-14);
        assert_eq!(binom_cdf(10, 10, 0.3), 1.0);
        assert_eq!(binom_cdf(0, 10, 0.0), 1.0);
        assert_eq!(binom_cdf(0, 10, 1.0), 0.0);
        // Large n stays finite.
        let c = binom_cdf(500, 1_000_000, 0.0005);
        assert!(c > 0.4 && c < 0.6, "{c}");
    }

    #[test]
    fn wilson_matches_reference_values() {
        // k=0, n=10 -> (0, 0.2775...); k=5, n=10 -> (0.2366, 0.7634) (standard references).
        let (lo, hi) = wilson_95(0, 10);
        assert!(lo == 0.0 && (hi - 0.27753).abs() < 2e-4, "{lo} {hi}");
        let (lo, hi) = wilson_95(5, 10);
        assert!(
            (lo - 0.23659).abs() < 2e-4 && (hi - 0.76341).abs() < 2e-4,
            "{lo} {hi}"
        );
    }

    #[test]
    fn bootstrap_is_seeded_and_brackets_the_mean() {
        let mut r = SplitMix64(9);
        let v: Vec<f64> = (0..400).map(|_| 0.9 + 0.1 * r.unit()).collect();
        let a = bootstrap_mean_ci(&v, 500, 42, 0.95).expect("non-empty");
        let b = bootstrap_mean_ci(&v, 500, 42, 0.95).expect("non-empty");
        assert_eq!(a, b);
        assert_ne!(a, bootstrap_mean_ci(&v, 500, 43, 0.95).expect("non-empty"));
        let m = mean(&v);
        assert!(a.0 < m && m < a.1);
        // The interval is about 2 * 1.96 * sd / sqrt(n); sd of U(0.9, 1.0) = 0.0289.
        let width = a.1 - a.0;
        assert!(
            (width - 2.0 * 1.96 * 0.0289 / 20.0).abs() < 0.0015,
            "{width}"
        );
        assert!(bootstrap_mean_ci(&[], 10, 1, 0.95).is_none());
        // A constant sample has a zero-width interval.
        let c = bootstrap_mean_ci(&[0.5; 30], 100, 1, 0.95).expect("non-empty");
        assert_eq!(c, (0.5, 0.5));
    }

    #[test]
    fn splitmix_matches_the_reference_sequence() {
        // First outputs for seed 0, from the published SplitMix64 reference implementation.
        let mut r = SplitMix64(0);
        assert_eq!(r.next_u64(), 0xE220_A839_7B1D_CDAF);
        assert_eq!(r.next_u64(), 0x6E78_9E6A_A1B9_65F4);
        assert_eq!(r.next_u64(), 0x06C4_5D18_8009_454F);
    }
}
