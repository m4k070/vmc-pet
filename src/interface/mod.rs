//! IF層。外界からの働きかけを、体・echo が受け取れる形へ翻訳するだけの層。
//!
//! ここが体に対してできることは `BodyPort`(と、環境シグナル専用の
//! `LeniaBody::apply_environmental_stress`)が許すことだけで、
//! 場を直接書き換える経路は持たない。
//!
//! - pointer.rs: ユーザーのポインタ操作 → 摂動(`Perturbation`)
//! - machine_load.rs: 機械の CPU 負荷 → 環境ストレス(スカラー)

pub mod machine_load;
pub mod pointer;

pub use machine_load::MachineLoad;
pub use pointer::{body_perturbation_for, echo_perturbation_for, Touch};
