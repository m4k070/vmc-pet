//! 1つのセルがどう見えるか(ドットの大きさ・不透明度・色)。PC と M5Stack で共有する。
//!
//! 以前は PC版(`src/render/dot_grid.rs`)と M5Stack版(`m5stack-cores3/src/bin/main.rs`)が
//! それぞれ色の定数と混ぜ方を持ち、「PC版と同じ狙い」とコメントで揃えているだけだった。
//! 実際に比べると、**体の色が M5Stack だけ約2割暗く**、色素の色も少しずれていた。
//! どちらも意図した調整ではなく、0〜1 の色を RGB565 へ手で写したときのずれだった
//! (M5Stack の体の色は、画面を目で確認できない状態で書かれていた)。
//!
//! 見た目の決まりは体を差し替えても変わらない普遍的な部分なので、ここに一本化した。
//! 各プラットフォームに残すのは、ピクセル形式への変換(ARGB8888 / RGB565)と
//! ドットの配置だけ。
//!
//! 決まりそのもの:
//!
//! - ドットの大きさは体と echo の値のうち大きい方で決める。面積が値に比例するよう
//!   平方根をとる
//! - 色は、まず色素の濃さに応じて体の色(ティール)を珊瑚色へ寄せ、その上に echo が
//!   占める割合で echo の色(暖色の白)を混ぜる
//! - 色素はドットの大きさを変えない。体も echo も無いところは、色素があっても描かない

use crate::math::sqrtf;

/// 0.0..=1.0 の RGB。ピクセル形式への変換は各プラットフォームが行う。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color {
    pub red: f32,
    pub green: f32,
    pub blue: f32,
}

impl Color {
    pub const fn new(red: f32, green: f32, blue: f32) -> Self {
        Self { red, green, blue }
    }

    /// `other` へ `amount`(0.0..=1.0)だけ寄せた色。
    pub fn toward(self, other: Color, amount: f32) -> Color {
        let amount = amount.clamp(0.0, 1.0);
        let mix = |from: f32, to: f32| from + (to - from) * amount;
        Color::new(
            mix(self.red, other.red),
            mix(self.green, other.green),
            mix(self.blue, other.blue),
        )
    }
}

/// 体そのものの色(ティール)。
pub const BODY_COLOR: Color = Color::new(0.35, 0.85, 0.80);

/// 触れた跡(echo)の色(暖色の白)。体の色とはっきり区別がつくよう、あえて系統を変える。
/// 「これは体の状態ではなく、触れた跡だ」と読み取れることを狙う。
pub const ECHO_COLOR: Color = Color::new(1.0, 0.92, 0.70);

/// 体に付く色素の色(珊瑚色)。世話を待っているときに体がこの色へ寄る。
/// 体のティールとも echo の暖色の白とも区別がつく系統にしてある。
pub const PIGMENT_COLOR: Color = Color::new(1.0, 0.55, 0.45);

/// ドット同士が接触しないよう、セル幅に対して空ける隙間の割合。
pub const DOT_GAP_RATIO: f32 = 0.18;

/// これ以下の値のセルは描画しない(ほぼ見えないドットを描く無駄を省く)。
pub const MIN_VISIBLE_VALUE: f32 = 0.004;

/// 1つのセルの見え方。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CellAppearance {
    /// ドットの大きさ。一番大きいドットの半径に対する比。
    pub size: f32,
    /// 不透明度。アルファを扱える表示だけが使う(M5Stack の RGB565 は不透明で描く)。
    pub opacity: f32,
    pub color: Color,
}

/// 体・echo・色素の値から、そのセルの見え方を決める。見えないセルなら `None`。
pub fn appearance_of(body: f32, echo: f32, pigment: f32) -> Option<CellAppearance> {
    let visibility = body.max(echo);
    if visibility <= MIN_VISIBLE_VALUE {
        return None;
    }
    // echo が占める割合。体だけなら 0、echo だけなら 1 になる
    let echo_share = (echo / visibility).clamp(0.0, 1.0);
    let body_color = BODY_COLOR.toward(PIGMENT_COLOR, pigment);
    Some(CellAppearance {
        size: sqrtf(visibility),
        opacity: visibility,
        color: body_color.toward(ECHO_COLOR, echo_share),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Color, b: Color) -> bool {
        (a.red - b.red).abs() < 1e-5
            && (a.green - b.green).abs() < 1e-5
            && (a.blue - b.blue).abs() < 1e-5
    }

    #[test]
    fn a_nearly_empty_cell_is_not_drawn() {
        // Arrange / Act / Assert
        assert_eq!(appearance_of(0.0, 0.0, 0.0), None);
        assert_eq!(appearance_of(MIN_VISIBLE_VALUE, 0.0, 0.0), None);
    }

    #[test]
    fn pigment_alone_never_makes_a_cell_visible() {
        // Arrange / Act / Assert: 体も echo も無いところの色素は描かない
        assert_eq!(appearance_of(0.0, 0.0, 1.0), None);
    }

    #[test]
    fn a_body_only_cell_has_the_body_color_and_an_area_proportional_to_its_value() {
        // Arrange / Act
        let appearance = appearance_of(0.25, 0.0, 0.0).expect("a body cell must be drawn");

        // Assert
        assert!(close(appearance.color, BODY_COLOR));
        assert!(
            (appearance.size - 0.5).abs() < 1e-6,
            "size is the square root of the value"
        );
        assert_eq!(appearance.opacity, 0.25);
    }

    #[test]
    fn an_echo_only_cell_has_the_echo_color() {
        // Arrange / Act
        let appearance = appearance_of(0.0, 0.6, 0.0).expect("an echo cell must be drawn");

        // Assert
        assert!(close(appearance.color, ECHO_COLOR));
    }

    #[test]
    fn a_fully_flushed_body_has_the_pigment_color() {
        // Arrange / Act
        let appearance = appearance_of(0.8, 0.0, 1.0).expect("a body cell must be drawn");

        // Assert
        assert!(close(appearance.color, PIGMENT_COLOR));
    }

    #[test]
    fn the_echo_is_mixed_on_top_of_the_flushed_body() {
        // Arrange / Act: 色づいた体に、体の半分の強さの echo
        let appearance = appearance_of(1.0, 0.5, 1.0).expect("a body cell must be drawn");

        // Assert: 珊瑚色と echo の色のちょうど中間
        assert!(close(
            appearance.color,
            PIGMENT_COLOR.toward(ECHO_COLOR, 0.5)
        ));
    }
}
