//! ポインタ操作を摂動へ翻訳する。
//!
//! 画面座標から場のセルへの変換は描画側(`render::DotGrid::cell_at`)が持つ。
//! ここが扱うのは「触れられたことが体・見た目それぞれにとって何を意味するか」だけ。
//!
//! 体への効果(body)と見た目の echo への効果(echo)を別関数に分けているのは、
//! docs/DESIGN.md に記録した通り、体を安全に保つ強さと、見た目としてはっきり
//! 分かる強さが両立しなかったため。撫でている間だけ弱く体に注入し続ける旧実装は、
//! 見えるほど強くすると体を殺し、かつクリックと併用すると安全境界が単調にならない
//! (履歴依存でしか生死が決まらない)ことが実測で分かった。
//!
//! そこで **ホバーは体には一切触れず、echo だけを動かす** ことにした。体に実際に
//! 影響を与えるのはクリックだけになる。これにより、ホバーとクリックの組み合わせが
//! 作っていた非単調な崩壊リスクは、経路そのものが無くなることで構造的に消える。

use crate::{CellPos, Perturbation};

/// ホバーは撫でている状態とみなす。体には触れず、echo だけを動かす。
/// 半径・強さは見た目の都合だけで決めてよく、体の安全域とは無関係。
const HOVER_ECHO_RADIUS_CELLS: f32 = 2.0;
const HOVER_ECHO_AMOUNT_PER_FRAME: f32 = 0.08;

/// クリックは突いた状態とみなす。体への実注入と、echo への強い単発フラッシュの両方を返す。
/// body 側の強さは、64箇所への注入で900ステップ後の生存を確認した安全域
/// (docs/DESIGN.md「摂動の強さと、体の崩壊からの復帰」参照)の内側に採ってある。
const CLICK_BODY_RADIUS_CELLS: f32 = 4.5;
const CLICK_BODY_AMOUNT: f32 = 0.20;

/// echo 側は見た目のためだけなので、体の安全域と無関係に強くしてよい。
const CLICK_ECHO_RADIUS_CELLS: f32 = 4.0;
const CLICK_ECHO_AMOUNT: f32 = 1.0;

/// ポインタが体に触れたことの意味。生の座標ではなく意図の単位で持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Touch {
    /// 撫でている。体には触れない。echo だけが触れている間ずっと反応する。
    Hover { at: CellPos },
    /// 突いた。体・echo の両方にその瞬間だけ強く注入する。
    Click { at: CellPos },
    /// 離れた。どちらにも何も起きない。
    Leave,
}

/// 体に実際に注入する摂動。ホバー単独では体に影響を与えない。
pub fn body_perturbation_for(touch: Touch) -> Option<Perturbation> {
    match touch {
        Touch::Click { at } => Some(Perturbation {
            at,
            radius: CLICK_BODY_RADIUS_CELLS,
            amount: CLICK_BODY_AMOUNT,
        }),
        Touch::Hover { .. } | Touch::Leave => None,
    }
}

/// 入力の可視化(echo)に加える摂動。ホバー・クリックの両方で `Some` を返す。
pub fn echo_perturbation_for(touch: Touch) -> Option<Perturbation> {
    match touch {
        Touch::Hover { at } => Some(Perturbation {
            at,
            radius: HOVER_ECHO_RADIUS_CELLS,
            amount: HOVER_ECHO_AMOUNT_PER_FRAME,
        }),
        Touch::Click { at } => Some(Perturbation {
            at,
            radius: CLICK_ECHO_RADIUS_CELLS,
            amount: CLICK_ECHO_AMOUNT,
        }),
        Touch::Leave => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hovering_never_touches_the_body() {
        // Arrange
        let at = CellPos { x: 4, y: 4 };

        // Act / Assert: ホバー単独では体への経路が存在しない
        assert!(body_perturbation_for(Touch::Hover { at }).is_none());
    }

    #[test]
    fn only_a_click_reaches_the_body() {
        // Arrange
        let at = CellPos { x: 4, y: 4 };

        // Act
        let perturbation = body_perturbation_for(Touch::Click { at }).unwrap();

        // Assert
        assert_eq!(perturbation.at, at);
        assert!(perturbation.amount > 0.0);
    }

    #[test]
    fn hovering_and_clicking_both_feed_the_echo() {
        // Arrange
        let at = CellPos { x: 4, y: 4 };

        // Act
        let hover = echo_perturbation_for(Touch::Hover { at }).unwrap();
        let click = echo_perturbation_for(Touch::Click { at }).unwrap();

        // Assert: クリックの方が echo でもはっきり強い(突いた瞬間のフラッシュ)
        assert!(click.amount > hover.amount);
    }

    #[test]
    fn leaving_touches_neither_the_body_nor_the_echo() {
        // Arrange / Act / Assert
        assert!(body_perturbation_for(Touch::Leave).is_none());
        assert!(echo_perturbation_for(Touch::Leave).is_none());
    }

    #[test]
    fn the_perturbation_lands_where_the_pointer_was() {
        // Arrange
        let at = CellPos { x: 9, y: 3 };

        // Act
        let body = body_perturbation_for(Touch::Click { at }).unwrap();
        let echo = echo_perturbation_for(Touch::Click { at }).unwrap();

        // Assert
        assert_eq!(body.at, at);
        assert_eq!(echo.at, at);
    }
}
