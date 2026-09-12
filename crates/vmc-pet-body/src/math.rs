//! 数学関数の薄いシム。
//!
//! `f32::sqrt` 等の超越関数は `core` には無く `std` にしか存在しない
//! (実測済み: `#![no_std]` で `x.sqrt()` はコンパイルエラーになる)。
//! 呼び出し側を `x.sqrt()` の代わりにここの `sqrtf(x)` を呼ぶ形にしておくことで、
//! `std` フィーチャの有無に関わらず同じソースを共有できる
//! (予測可能性: 同じ処理は同じパターンで書く)。
//!
//! `body` が実際に使う超越関数は sqrt/cos/exp/floor/ceil の5種類だけで、
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
pub fn expf(x: f32) -> f32 {
    x.exp()
}
#[cfg(not(feature = "std"))]
pub fn expf(x: f32) -> f32 {
    libm::expf(x)
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
}
