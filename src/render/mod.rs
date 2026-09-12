//! 体の状態をピクセルに落とすレイヤ。場の値以外の情報源を持たない。
//!
//! `TouchEcho` と `Camera`(表示の原点を重心へ寄せる)は `vmc_pet_body` 側に
//! ある(体を差し替えても変わらない普遍的なデータ・計算で、M5Stack 版とも
//! 共有する。docs/M5STACK.md 参照)。ここで持つのは、場・echo をどうピクセルへ
//! 描くか(Wayland の wl_shm・ARGB8888 固有の部分)だけ。

pub mod dot_grid;

pub use dot_grid::DotGrid;
pub use vmc_pet_body::{Camera, TouchEchoView};
