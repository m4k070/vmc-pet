//! 体の状態をピクセルに落とすレイヤ。場の値以外の情報源を持たない。
//!
//! `TouchEcho` は `vmc_pet_body` 側にある(体を差し替えても変わらない普遍的な
//! データで、M5Stack 版とも共有する。docs/M5STACK.md 参照)。ここで持つのは、
//! 場・echo をどうピクセルへ描くか(Wayland の wl_shm・ARGB8888 固有の部分)だけ。

pub mod camera;
pub mod dot_grid;

pub use camera::Camera;
pub use dot_grid::DotGrid;
pub use vmc_pet_body::{TouchEcho, TouchEchoView};
