//! IF層。外界からの働きかけを、体・echo が受け取れる形へ翻訳するだけの層。
//!
//! ここが体に対してできることは `BodyPort`(と、環境シグナル専用の
//! `LeniaBody::apply_environmental_stress`)が許すことだけで、
//! 場を直接書き換える経路は持たない。
//!
//! - `Touch` / `body_perturbation_for` / `echo_perturbation_for`:
//!   触れ方の意味づけそのものは `vmc_pet_body` 側にある(体を差し替えても
//!   変わらない普遍的な部分で、M5Stack 版のタッチ入力とも共有する。
//!   docs/M5STACK.md 参照)。ここでは再公開するだけ。
//! - machine_load.rs: 機械の CPU 負荷 → 環境ストレス(スカラー)。
//!   `/proc/stat` を読む OS 依存の部分なので、こちらはデスクトップ版だけの実装。

pub mod machine_load;

pub use machine_load::MachineLoad;
pub use vmc_pet_body::{body_perturbation_for, echo_perturbation_for, Touch};
