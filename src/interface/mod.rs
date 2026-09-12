//! IF層。外界からの働きかけを、体が受け取れる形へ翻訳するだけの層。
//!
//! 触れ方の意味づけ(`Touch` / `body_perturbation_for` / `echo_perturbation_for`)
//! と、体・入力の翻訳・崩壊検知・echo を束ねるオーケストレーション
//! (`vmc_pet_body::Pet`)は、体を差し替えても変わらない普遍的な部分として
//! `vmc_pet_body` 側にあり、M5Stack 版とも共有する(docs/M5STACK.md 参照)。
//! `app.rs` はそれを直接使うため、ここでの再公開は不要になった。
//!
//! - machine_load.rs: 機械の CPU 負荷 → 環境ストレス(スカラー)。
//!   `/proc/stat` を読む OS 依存の部分なので、こちらはデスクトップ版だけの実装。

pub mod machine_load;

pub use machine_load::MachineLoad;
