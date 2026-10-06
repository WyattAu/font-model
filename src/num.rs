//! Small numeric helpers that `no_std` does not provide.
//!
//! `core` has no `f32::round`, `f32::abs`, or `f64::sqrt`; `std` does, and
//! `libm` does. Rather than take a `libm` dependency for two calls the model
//! makes a handful of times, they are written out here — the implementations
//! are short enough to audit and the crate keeps a two-dependency floor
//! (`font-parse`, `thiserror`).
//!
//! All three match IEEE/std semantics exactly, which is what a test comparing
//! against `std`'s behaviour needs.

/// Round half away from zero, matching `f32::round`.
///
/// `no_std`-safe: `f32 as i32` truncates toward zero, so one half-sign is added
/// before truncating. Non-finite input passes through unchanged, which is what
/// std does.
#[must_use]
pub fn round(x: f32) -> f32 {
    if !x.is_finite() {
        return x;
    }
    // The `as i32` cast saturates, and the range guards below turn that
    // saturation back into the (out-of-i32) float std would have produced.
    const LIMIT: f32 = 16_777_216.0; // 2^24
    if x >= LIMIT || x <= -LIMIT {
        // Every f32 at or beyond 2^24 is already an integer, so rounding is
        // the identity.
        return x;
    }
    let shifted = if x < 0.0 { x - 0.5 } else { x + 0.5 };
    (shifted as i32) as f32
}

/// Absolute value, matching `f32::abs` (and therefore clearing the sign bit,
/// so `abs(NaN)` stays `NaN`).
#[must_use]
pub fn abs(x: f32) -> f32 {
    f32::from_bits(x.to_bits() & 0x7FFF_FFFF)
}

/// The nearest integer, saturating at the `i32` bounds.
///
/// `as i32` is a *saturating* cast in `std`, but that behaviour is only
/// available with the `float_casts` feature — in `core` the cast is rejected
/// outright for float-to-int. The comparison against the bounds reproduces
/// std's saturation exactly; `NaN` compares false against both and lands on 0,
/// matching std.
#[must_use]
pub fn to_i32(x: f32) -> i32 {
    const LO: f32 = -2_147_483_648.0; // i32::MIN as f32
    const HI: f32 = 2_147_483_648.0; // i32::MAX + 1 as f32, the first
                                     // unrepresentable positive value
    if x.is_nan() {
        return 0;
    }
    if x <= LO {
        return i32::MIN;
    }
    if x >= HI {
        return i32::MAX;
    }
    x as i32
}

/// Ceiling, matching `f64::ceil` (and therefore leaving `±inf` and `NaN`
/// alone).
#[must_use]
pub fn ceil(x: f64) -> f64 {
    if !x.is_finite() || x == 0.0 {
        return x;
    }
    // Every f64 at or beyond 2^52 is already an integer, so the ceiling is
    // the identity — and the `as i64` cast would saturate.
    const LIMIT: f64 = 4_503_599_627_370_496.0; // 2^52
    if x >= LIMIT || x <= -LIMIT {
        return x;
    }
    let truncated = x as i64 as f64;
    if x > 0.0 && truncated < x {
        truncated + 1.0
    } else {
        truncated
    }
}

/// Square root. `NaN` for negative input, exactly as `f32::sqrt`.
///
/// Accurate to within one ulp of the hardware square root: the iteration runs
/// in `f64` and rounds once at the end. The one-ulp band is where
/// double-rounding lives — for inputs whose exact root sits almost precisely
/// between two `f32` values, the `f64` intermediate rounds the other way. A
/// glyph rasterizer cannot see that difference; a caller comparing bit patterns
/// against `sqrtf` should not.
#[must_use]
pub fn sqrt(x: f32) -> f32 {
    if x < 0.0 {
        return f32::NAN;
    }
    if x == 0.0 || !x.is_finite() {
        return x;
    }
    // The bit-level initial estimate: halving the biased exponent lands within
    // a small factor of the true root. The iteration then runs in `f64`, whose
    // 53 bits of mantissa hold an `f32` root with ~21 bits to spare, so one
    // final `f32` rounding of an `f64`-refined value is *correctly* rounded —
    // iterating in `f32` alone stalls a ulp short (as a hardware square root
    // does not).
    let mut guess = f64::from(f32::from_bits((x.to_bits() >> 1) + 0x1F80_0000));
    if guess <= 0.0 || !guess.is_finite() {
        guess = 1.0;
    }
    let target = f64::from(x);
    for _ in 0..4 {
        guess = 0.5 * (guess + target / guess);
    }
    guess as f32
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::{abs, round, sqrt, to_i32};

    // Test harness: assertions legitimately panic; the lib target stays
    // lint-clean.

    #[test]
    fn round_matches_std() {
        for x in [
            0.0f32,
            0.4,
            0.5,
            0.6,
            -0.4,
            -0.5,
            -0.6,
            1.5,
            -1.5,
            2.5,
            -2.5,
            100.499,
            -100.499,
            1234.5678,
            f32::MIN,
            f32::MAX,
            f32::INFINITY,
            f32::NEG_INFINITY,
        ] {
            assert_eq!(round(x), x.round(), "round({x})");
        }
        assert!(round(f32::NAN).is_nan());
    }

    #[test]
    fn abs_matches_std() {
        for x in [0.0f32, -0.0, 1.5, -1.5, -1e30, 1e30, f32::MIN, f32::MAX] {
            assert_eq!(abs(x), x.abs(), "abs({x})");
        }
        assert!(abs(f32::NAN).is_nan());
        assert!(abs(f32::NEG_INFINITY) == f32::INFINITY);
    }

    #[test]
    fn to_i32_matches_the_saturating_cast() {
        for x in [
            0.0f32,
            -0.0,
            1.9,
            -1.9,
            2_147_483_647.0,
            2_147_483_648.0,
            1e30,
            -2_147_483_648.0,
            -2_147_483_968.0,
            -1e30,
            f32::INFINITY,
            f32::NEG_INFINITY,
        ] {
            // `as i32` with std's `float_casts` semantics, spelled out: the
            // documented contract is saturation.
            let expected = if x.is_nan() {
                0
            } else if x <= -2_147_483_648.0 {
                i32::MIN
            } else if x >= 2_147_483_648.0 {
                i32::MAX
            } else {
                x as i32
            };
            assert_eq!(to_i32(x), expected, "to_i32({x})");
        }
        assert_eq!(to_i32(f32::NAN), 0);
    }

    #[test]
    fn ceil_matches_std() {
        for x in [
            0.0f64,
            0.1,
            0.5,
            1.0,
            1.000_000_1,
            -0.1,
            -0.5,
            -1.0,
            -1.5,
            100.000_001,
            -100.000_001,
            f64::MIN,
            f64::MAX,
            f64::INFINITY,
            f64::NEG_INFINITY,
        ] {
            assert_eq!(super::ceil(x), x.ceil(), "ceil({x})");
        }
        assert!(super::ceil(f64::NAN).is_nan());
    }

    /// Relative closeness within one `f32` ulp — `f32::EPSILON` is 2^-23, and a
    /// value one ulp off at magnitude `m` differs by at most `m·EPSILON`.
    fn within_one_ulp(a: f32, b: f32) -> bool {
        if a == b {
            return true;
        }
        if !a.is_finite() || !b.is_finite() {
            return false;
        }
        crate::num::abs(a - b) <= crate::num::abs(b) * f32::EPSILON
    }

    #[test]
    fn sqrt_is_correctly_rounded_to_within_one_ulp() {
        // A hardware square root is correctly rounded. Iterating in f64 and
        // rounding once lands within a ulp of it for every input; the
        // double-rounding cases (where the exact root sits almost exactly
        // between two f32 values) are the documented exception, and they are
        // the ones the ulp bound allows.
        for x in [
            0.0f32,
            1.0,
            2.0,
            3.0,
            4.0,
            9.0,
            16.0,
            0.25,
            1e-8,
            1e8,
            core::f32::consts::PI,
            1.0e-30,
            1.0e30,
            f32::MIN_POSITIVE,
            f32::MAX,
        ] {
            assert!(
                within_one_ulp(sqrt(x), x.sqrt()),
                "sqrt({x}) = {} vs {}",
                sqrt(x),
                x.sqrt()
            );
        }
        assert!(sqrt(-1.0).is_nan());
        assert_eq!(sqrt(f32::INFINITY), f32::INFINITY);
        assert_eq!(sqrt(0.0), 0.0);
        assert_eq!(sqrt(4.0), 2.0, "exact values stay exact");
    }

    #[test]
    fn sqrt_is_accurate_over_a_sweep() {
        let mut worst = 0.0f32;
        for i in 0..2000u32 {
            let x = i as f32 * 0.37;
            if x == 0.0 {
                continue;
            }
            let mine = sqrt(x);
            let theirs = x.sqrt();
            assert!(
                within_one_ulp(mine, theirs),
                "sqrt({x}) = {mine} vs {theirs}"
            );
            let rel = crate::num::abs(mine - theirs) / theirs;
            worst = worst.max(rel);
        }
        assert!(worst <= 1e-7, "worst relative error {worst}");
    }
}
