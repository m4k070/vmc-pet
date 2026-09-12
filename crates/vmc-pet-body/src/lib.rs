//! 体そのもの。Wayland も入力も知らない純粋なデータ構造とロジックだけを置く。
//!
//! `std` フィーチャ(デフォルト有効)を切ると `no_std` になる。デスクトップ版
//! (vmc-pet 本体)は `std` のまま使い、組み込み向けの体(docs/M5STACK.md)は
//! これを切って再利用する。数学関数だけ `math` モジュールの薄いシムを介するため、
//! `field.rs` / `lenia.rs` / `perturbation.rs` 自体は書き換えなくてよい。
//!
//! `touch` / `touch_echo` もここに置く。触れ方の意味づけ(体には触れないホバー、
//! 安全域の内側に採ったクリックの強さなど)と、入力の可視化(echo)の減衰・合成の
//! ロジックは、体を差し替えても変わらない普遍的な部分であり、デスクトップ版と
//! 組み込み版(M5Stack)の両方が同じ実装・同じ安全性検証済みの定数を共有する。
//! 画面へどう描くか(ピクセルフォーマット・座標変換)だけが、体ごとに別々の
//! render 層の仕事として残る。

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

mod math;

pub mod animal;
pub mod controller;
pub mod field;
pub mod lenia;
pub mod lenia_body;
pub mod perturbation;
pub mod pet;
pub mod port;
pub mod touch;
pub mod touch_echo;

pub use animal::{list_animals, load_animal, Animal};
pub use controller::AutonomousController;
pub use field::{Field, FieldView};
pub use lenia::Lenia;
pub use lenia_body::LeniaBody;
pub use perturbation::{accumulate_into, CellPos, Perturbation};
pub use pet::Pet;
pub use port::BodyPort;
pub use touch::{body_perturbation_for, echo_perturbation_for, Touch};
pub use touch_echo::{TouchEcho, TouchEchoView};
