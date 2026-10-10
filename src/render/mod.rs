//! 体の状態をピクセルに落とすレイヤ。場の値以外の情報源を持たない。
//!
//! `TouchEcho` と `Camera`(表示の原点を重心へ寄せる)は `vmc_pet_body` 側に
//! ある(体を差し替えても変わらない普遍的なデータ・計算で、M5Stack 版とも
//! 共有する。docs/M5STACK.md 参照)。ここで持つのは、場・echo をどうピクセルへ
//! 描くか(Wayland の wl_shm・ARGB8888 固有の部分)だけ。
//!
//! `pose` は姿勢(ジェスチャー)の投影。姿勢そのもの(語彙と写像)は体の側
//! (`vmc_pet_body::gesture`)にあり、ここにあるのは「ドットグリッドへどう写すか」。

pub mod dot_grid;
pub mod pose;

pub use dot_grid::DotGrid;
pub use pose::PoseProjection;
pub use vmc_pet_body::{Camera, TouchEchoView};
