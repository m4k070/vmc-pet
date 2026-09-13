//! 数学関数の薄いシム。
//!
//! `f32::sqrt` 等の超越関数は `core` には無く `std` にしか存在しない
//! (実測済み: `#![no_std]` で `x.sqrt()` はコンパイルエラーになる)。
//! 呼び出し側を `x.sqrt()` の代わりにここの `sqrtf(x)` を呼ぶ形にしておくことで、
//! `std` フィーチャの有無に関わらず同じソースを共有できる
//! (予測可能性: 同じ処理は同じパターンで書く)。
//!
//! `body` が実際に使う超越関数は sqrt/sin/cos/atan2/exp/floor/ceil の7種類だけで、
//! `powi` は指数が定数(4)のときの掛け算に展開して済ませている。

#[cfg(feature = "std")]
pub fn sqrtf(x: f32) -> f32 {
    x.sqrt()
}
#[cfg(not(feature = "std"))]
pub fn sqrtf(x: f32) -> f32 {
    libm::sqrtf(x)
}

#[cfg(feature = "std")]
pub fn cosf(x: f32) -> f32 {
    x.cos()
}
#[cfg(not(feature = "std"))]
pub fn cosf(x: f32) -> f32 {
    libm::cosf(x)
}

#[cfg(feature = "std")]
pub fn sinf(x: f32) -> f32 {
    x.sin()
}
#[cfg(not(feature = "std"))]
pub fn sinf(x: f32) -> f32 {
    libm::sinf(x)
}

#[cfg(feature = "std")]
pub fn atan2f(y: f32, x: f32) -> f32 {
    y.atan2(x)
}
#[cfg(not(feature = "std"))]
pub fn atan2f(y: f32, x: f32) -> f32 {
    libm::atan2f(y, x)
}

#[cfg(feature = "std")]
pub fn expf(x: f32) -> f32 {
    x.exp()
}
#[cfg(not(feature = "std"))]
pub fn expf(x: f32) -> f32 {
    libm::expf(x)
}

#[cfg(feature = "std")]
pub fn lnf(x: f32) -> f32 {
    x.ln()
}
#[cfg(not(feature = "std"))]
pub fn lnf(x: f32) -> f32 {
    libm::logf(x)
}

#[cfg(feature = "std")]
pub fn floorf(x: f32) -> f32 {
    x.floor()
}
#[cfg(not(feature = "std"))]
pub fn floorf(x: f32) -> f32 {
    libm::floorf(x)
}

#[cfg(feature = "std")]
pub fn ceilf(x: f32) -> f32 {
    x.ceil()
}
#[cfg(not(feature = "std"))]
pub fn ceilf(x: f32) -> f32 {
    libm::ceilf(x)
}

/// `x` の小数部分。呼び出し側では常に `x >= 0.0` の場面でしか使わないため、
/// `x - floor(x)` で十分(負値の丸め方向の違いは考慮していない)。
pub fn fractf(x: f32) -> f32 {
    x - floorf(x)
}

/// `x` を `0.0..y` に折り返す(`f32::rem_euclid` 相当。これも `core` には無く
/// `std` にしか無いことを実測で確認済み)。呼び出し側では常に `y > 0.0` の場面
/// (角度の一周・場の幅/高さなど)でしか使わないため、その前提で十分。
/// `%` 演算子は `core` でも使える組み込み演算なので、それだけで済む。
pub fn rem_euclidf(x: f32, y: f32) -> f32 {
    let remainder = x % y;
    if remainder < 0.0 {
        remainder + y
    } else {
        remainder
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqrtf_matches_the_standard_library() {
        assert!((sqrtf(4.0) - 2.0).abs() < 1e-6);
    }

    #[test]
    fn cosf_matches_the_standard_library() {
        assert!((cosf(0.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn sinf_matches_the_standard_library() {
        assert!((sinf(0.0) - 0.0).abs() < 1e-6);
    }

    #[test]
    fn atan2f_matches_the_standard_library() {
        assert!((atan2f(1.0, 1.0) - core::f32::consts::FRAC_PI_4).abs() < 1e-6);
    }

    #[test]
    fn expf_matches_the_standard_library() {
        assert!((expf(0.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn floorf_and_ceilf_match_the_standard_library() {
        assert_eq!(floorf(1.7), 1.0);
        assert_eq!(ceilf(1.2), 2.0);
    }

    #[test]
    fn fractf_returns_the_fractional_part() {
        assert!((fractf(2.75) - 0.75).abs() < 1e-6);
    }

    #[test]
    fn rem_euclidf_matches_the_standard_library() {
        assert!((rem_euclidf(5.5, 2.0) - 1.5).abs() < 1e-6);
        assert!((rem_euclidf(-0.5, 2.0) - 1.5).abs() < 1e-6, "negative input must wrap positive");
    }
}
