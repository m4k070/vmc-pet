//! IF層。外界からの働きかけを、体が受け取れる摂動へ翻訳するだけの層。
//!
//! ここが体に対してできることは `BodyPort` が許すことだけで、
//! 場を直接書き換える経路は持たない。

pub mod pointer;

pub use pointer::{perturbation_for, Touch};
