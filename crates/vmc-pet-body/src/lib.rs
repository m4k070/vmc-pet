//! 体そのもの。Wayland も入力も知らない純粋なデータ構造とロジックだけを置く。
//!
//! `std` フィーチャ(デフォルト有効)を切ると `no_std` になる。デスクトップ版
//! (vmc-pet 本体)は `std` のまま使い、組み込み向けの体(docs/M5STACK.md)は
//! これを切って再利用する。数学関数だけ `math` モジュールの薄いシムを介するため、
//! `field.rs` / `lenia.rs` / `perturbation.rs` 自体は書き換えなくてよい。

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

mod math;

pub mod animal;
pub mod field;
pub mod lenia;
pub mod lenia_body;
pub mod perturbation;
pub mod port;

pub use animal::{list_animals, load_animal, Animal};
pub use field::{Field, FieldView};
pub use lenia::Lenia;
pub use lenia_body::LeniaBody;
pub use perturbation::{accumulate_into, CellPos, Perturbation};
pub use port::BodyPort;
