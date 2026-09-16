//! M5Stack CoreS3 側のコントローラ層のうち、`main` から切り出せる部分。
//!
//! 世界モデル(体・入力の翻訳・崩壊検知・echo)は `vmc_pet_body` にあり、
//! PC版と共有している。ここに置くのは「このハードウェアでしか意味を持たない
//! 部分」で、時計(RTC)と記憶の置き場所(フラッシュ)。加えて、【実験】多チャンネルの生物を
//! 体として動かすモードの描き方(`multichannel`)。
//!
//! `src/bin/main.rs` が直接 `mod` で抱えるのではなくライブラリにしてあるのは、
//! 画面への描き方(`DotRenderer`)と、周辺機器の面倒(時計・フラッシュ)を
//! 別のファイルに分けて読めるようにするため。

#![no_std]

extern crate alloc;

pub mod clock;
pub mod multichannel;
pub mod persistence;
