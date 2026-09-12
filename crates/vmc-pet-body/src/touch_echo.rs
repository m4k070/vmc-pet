//! 入力を可視化するためだけのデータ。体の Field とは完全に別に持つ。
//!
//! 「触れたことが体に本当に影響したか」(body への注入)と「画面上でどう見えるか」
//! (ここでの echo)を分離する。これにより、見た目の演出の強さを体の安定性と無関係に
//! 決められる。ホバーで光らせようとすると体を殺してしまう、という問題
//! (docs/DESIGN.md「ホバーで光らせることはできない」を参照)は、ホバーが体の場に
//! 一切触れなくなることで構造的に解消される。
//!
//! Lenia のような成長規則は持たない。指数減衰するだけの、純粋な描画用の記憶。
//!
//! なお、この分離は副次的に、体を差し替える将来の拡張(docs/DESIGN.md「将来の拡張」)で
//! 世界モデル的なものを足す際に必要になる「行動のコピー」の置き場にもなりうる。
//! いまは render 層に閉じた描画専用データだが、場(z 相当)と行動(a 相当)を最初から
//! 別データとして持っているという構造そのものは、その拡張と衝突しない。

use alloc::vec;
use alloc::vec::Vec;

use crate::{accumulate_into, Perturbation};

/// heat の値域。Field の値域(0.0..=1.0)と揃えてある。
const MIN_HEAT: f32 = 0.0;
const MAX_HEAT: f32 = 1.0;

/// 触れた跡の記憶。体には一切影響しない。
pub struct TouchEcho {
    width: usize,
    height: usize,
    heat: Vec<f32>,
}

impl TouchEcho {
    /// すべてのセルが 0.0 の echo を作る。
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            heat: vec![MIN_HEAT; width * height],
        }
    }

    /// 触れた位置に加算する。`Field::inject` と同じ山型・トーラス折り返しを共有する。
    pub fn touch(&mut self, perturbation: &Perturbation) {
        accumulate_into(
            &mut self.heat,
            self.width,
            self.height,
            perturbation,
            MIN_HEAT,
            MAX_HEAT,
        );
    }

    /// 毎フレーム呼ぶ。全体を指数減衰させるだけで、Lenia のような成長規則は持たない。
    pub fn decay(&mut self, factor: f32) {
        for value in &mut self.heat {
            *value *= factor;
        }
    }

    /// 読み取り専用ビューを返す。
    pub fn view(&self) -> TouchEchoView<'_> {
        TouchEchoView {
            width: self.width,
            height: self.height,
            heat: &self.heat,
        }
    }
}

/// echo の読み取り専用ビュー。
#[derive(Debug, Clone, Copy)]
pub struct TouchEchoView<'a> {
    width: usize,
    height: usize,
    heat: &'a [f32],
}

impl TouchEchoView<'_> {
    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    /// 範囲外の座標は 0.0 を返す。
    pub fn get(&self, x: usize, y: usize) -> f32 {
        if x >= self.width || y >= self.height {
            return MIN_HEAT;
        }
        self.heat[y * self.width + x]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CellPos;

    #[test]
    fn touch_raises_the_centre_most() {
        // Arrange
        let mut echo = TouchEcho::new(16, 16);

        // Act
        echo.touch(&Perturbation {
            at: CellPos { x: 8, y: 8 },
            radius: 3.0,
            amount: 0.5,
        });

        // Assert
        let view = echo.view();
        assert!((view.get(8, 8) - 0.5).abs() < 1e-5);
        assert!(view.get(9, 8) < view.get(8, 8));
        assert_eq!(view.get(12, 8), 0.0);
    }

    #[test]
    fn decay_shrinks_every_cell_by_the_same_factor() {
        // Arrange
        let mut echo = TouchEcho::new(4, 4);
        echo.touch(&Perturbation {
            at: CellPos { x: 2, y: 2 },
            radius: 2.0,
            amount: 1.0,
        });
        let before = echo.view().get(2, 2);

        // Act
        echo.decay(0.5);

        // Assert
        assert!((echo.view().get(2, 2) - before * 0.5).abs() < 1e-5);
    }

    #[test]
    fn continuous_touching_settles_below_the_cap_instead_of_saturating() {
        // Arrange: 毎フレーム同じ位置へ加算しつつ減衰させる、ホバーを固定した状況を模す
        let mut echo = TouchEcho::new(16, 16);
        let at = CellPos { x: 8, y: 8 };

        // Act: 120フレーム(30fpsで4秒ぶん)撫で続ける
        for _ in 0..120 {
            echo.touch(&Perturbation {
                at,
                radius: 2.0,
                amount: 0.08,
            });
            echo.decay(0.90);
        }

        // Assert: 上限に張り付かず、はっきり見える定常値に落ち着く
        let steady = echo.view().get(8, 8);
        assert!(steady < MAX_HEAT, "must not saturate, got {steady}");
        assert!(steady > 0.3, "must be clearly visible, got {steady}");
    }

    #[test]
    fn the_glow_fades_within_a_couple_of_seconds_after_release() {
        // Arrange: 定常状態(0.08/(1-0.90) = 0.8 付近)まで撫でてから離す
        let mut echo = TouchEcho::new(16, 16);
        let at = CellPos { x: 8, y: 8 };
        for _ in 0..120 {
            echo.touch(&Perturbation {
                at,
                radius: 2.0,
                amount: 0.08,
            });
            echo.decay(0.90);
        }

        // Act: 触れるのをやめ、30fps で40フレーム(約1.3秒)ぶん減衰だけ進める。
        // 0.8 * 0.90^40 ≈ 0.012 になる計算で、30フレーム(≈1秒)ではまだ 0.034 残り
        // 閾値に届かないため、実測に合わせて40フレームで確認する。
        for _ in 0..40 {
            echo.decay(0.90);
        }

        // Assert
        assert!(
            echo.view().get(8, 8) < 0.02,
            "the glow must fade within a couple of seconds after release"
        );
    }
}
