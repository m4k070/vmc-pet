//! ポインタ操作を摂動へ翻訳する。
//!
//! 画面座標から場のセルへの変換は描画側(`render::DotGrid::cell_at`)が持つ。
//! ここが扱うのは「触れられたことが体にとって何を意味するか」だけ。

use crate::body::{CellPos, Perturbation};

/// ホバーは撫でている状態とみなし、弱く注入し続ける。
/// 強さは実測で決めた。Lenia はカオス系で安全境界が単調にならないため、
/// この値でも状況によっては体が崩壊しうる。崩壊からの復帰は app 側が受け持つ。
const HOVER_RADIUS_CELLS: f32 = 2.5;
const HOVER_AMOUNT_PER_STEP: f32 = 0.02;

/// クリックは突いた状態とみなし、強く単発で注入する。
const CLICK_RADIUS_CELLS: f32 = 4.5;
const CLICK_AMOUNT: f32 = 0.20;

/// ポインタが体に触れたことの意味。生の座標ではなく意図の単位で持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Touch {
    /// 撫でている。触れている間ずっと弱く注入する。
    Hover { at: CellPos },
    /// 突いた。その瞬間だけ強く注入する。
    Click { at: CellPos },
    /// 離れた。何も注入しない。
    Leave,
}

/// 触れ方に対応する摂動を返す。離れた場合は何も起きない。
pub fn perturbation_for(touch: Touch) -> Option<Perturbation> {
    match touch {
        Touch::Hover { at } => Some(Perturbation {
            at,
            radius: HOVER_RADIUS_CELLS,
            amount: HOVER_AMOUNT_PER_STEP,
        }),
        Touch::Click { at } => Some(Perturbation {
            at,
            radius: CLICK_RADIUS_CELLS,
            amount: CLICK_AMOUNT,
        }),
        Touch::Leave => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_click_hits_harder_than_a_hover() {
        // Arrange
        let at = CellPos { x: 4, y: 4 };

        // Act
        let hover = perturbation_for(Touch::Hover { at }).unwrap();
        let click = perturbation_for(Touch::Click { at }).unwrap();

        // Assert
        assert!(click.amount > hover.amount);
        assert!(click.radius > hover.radius);
    }

    #[test]
    fn leaving_injects_nothing() {
        // Arrange / Act / Assert
        assert!(perturbation_for(Touch::Leave).is_none());
    }

    #[test]
    fn the_perturbation_lands_where_the_pointer_was() {
        // Arrange
        let at = CellPos { x: 9, y: 3 };

        // Act
        let perturbation = perturbation_for(Touch::Click { at }).unwrap();

        // Assert
        assert_eq!(perturbation.at, at);
    }
}
