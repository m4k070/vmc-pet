//! OS依存部分(wlr-layer-shell)を隔離するレイヤ。
//! この配下だけが Wayland を知っており、上位は [`Surface`] を通してしか触れない。

pub mod layer;

pub use layer::{LayerWindow, LayerWindowConfig};

/// 入力領域(クリック・ホバーを受け取る矩形)を論理ピクセルで表す。
/// この矩形の外側はクリックが下のウィンドウへ抜ける。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InputRegion {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// shell層が受け取ったポインタ操作を、Waylandの語彙から切り離して表したもの。
/// 座標はサーフェス左上を原点とする論理ピクセル。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PointerInput {
    Entered { x: f64, y: f64 },
    Moved { x: f64, y: f64 },
    Pressed { x: f64, y: f64 },
    Left,
}

/// shell層から描画内容と入力受理範囲を問い合わせるための窓口。
/// 実装側は Wayland を、shell層は体のロジックを、互いに知らないままでいられる。
pub trait Surface {
    /// 1フレーム分を `canvas` に描画する。
    /// `canvas` は premultiplied ARGB8888、行ストライドは `width * 4` バイト。
    fn draw(&mut self, canvas: &mut [u8], width: u32, height: u32);

    /// クリックを受け取る矩形を返す。ここ以外は下のウィンドウへ透過する。
    fn input_region(&self, width: u32, height: u32) -> InputRegion;

    /// ポインタ操作を受け取る。摂動の注入はここを起点にする。
    fn on_pointer(&mut self, input: PointerInput);
}
