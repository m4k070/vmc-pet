//! 摂動を可視化するためだけのデータ。体の Field とは完全に別に持つ。
//!
//! 当初は「入力(人間が触れたこと)の可視化」専用だったが、自律コントローラが
//! 自分から働きかけたことも同じ仕組みで光らせるようになったため、いまは
//! 「外から触れられた/自分で身じろぎした、どちらの摂動も可視化する場所」に
//! 意味が広がっている(`Pet::step` の `SELF_ACTION_ECHO_AMOUNT` 参照)。
//! 体への摂動を強めて動きを見せる道が、安定性の崖に阻まれて行き止まりだった
//! ため、ここでもう一度「見た目と体を分離する」手を使っている。
//!
//! 「触れたことが体に本当に影響したか」(body への注入)と「画面上でどう見えるか」
//! (ここでの echo)を分離する。これにより、見た目の演出の強さを体の安定性と無関係に
//! 決められる。ホバーで光らせようとすると体を殺してしまう、という問題
//! (docs/DESIGN.md「ホバーで光らせることはできない」を参照)は、ホバーが体の場に
//! 一切触れなくなることで構造的に解消される。
//!
//! Lenia のような成長規則は持たない。指数減衰するだけの、純粋な描画用の記憶。
//!
//! # 光は体(=カメラ)に貼りつける
//!
//! 光は場(世界)の座標ではなく、**体の重心からの相対位置**で覚える。当初は場の
//! 座標で覚えていたため、生物が進む(O2u は約9セル/秒)とカメラがそれを追い、
//! 触った場所の光が画面上を後ろへ流れて見えていた。カメラは重心を画面中央に置く
//! だけなので、体基準の座標はカメラの座標そのものであり、光は触った指の下に
//! 留まる。慣れ(`habituation`)を体の部位ごとに覚えるようにしたのと同じ理由。
//!
//! 体の重心は `follow_body` で受け取る。`Camera::follow` と同じ形にしてあるのは、
//! どちらも「体の重心を基準に座標をずらす」同じ操作だから。受け取らなければ
//! 原点(0, 0)のままで、場の座標で記録・読み出しするのと同じになる。
//!
//! 重心は小数で持ち、読み出し(`TouchEchoView::get`)のときに双線形補間する。
//! 整数セルに丸めて読むと、重心が1セル進むたびに光が最大±0.5セル跳ね、
//! 体全体で一度踏んだ「丸めによる揺れ」(camera.rs 参照)を光だけで再現してしまう。
//! 補間して読めば、読む位置は画面上の位置だけで決まり、光は滑らかに留まる。
//! 記録(`touch`)は1回きりなので、そこでの丸め(最大0.5セル)は揺れにならない。
//!
//! なお、この分離は副次的に、体を差し替える将来の拡張(docs/DESIGN.md「将来の拡張」)で
//! 世界モデル的なものを足す際に必要になる「行動のコピー」の置き場にもなりうる。
//! いまは render 層に閉じた描画専用データだが、場(z 相当)と行動(a 相当)を最初から
//! 別データとして持っているという構造そのものは、その拡張と衝突しない。

use alloc::vec;
use alloc::vec::Vec;

use crate::body_frame::{nearest_cell, sample_on_body};
use crate::{accumulate_into_toroidal, CellPos, Perturbation};

/// heat の値域。Field の値域(0.0..=1.0)と揃えてある。
const MIN_HEAT: f32 = 0.0;
const MAX_HEAT: f32 = 1.0;

/// 触れた跡の記憶。体には一切影響しない。
///
/// 内部の配列は体の重心を原点とした座標で並んでいる(`toroidal` が true のとき)。
/// `follow_body` を呼ばない体(壁で跳ね返る箱の体。粒子の体)では、重心が原点
/// (0,0)のままなので、場の座標で記録・読み出しする。そのとき折り返しも無効に
/// しないと、壁際に置かれた摂動の外周が場の反対側へ漏れて見える
pub struct TouchEcho {
    width: usize,
    height: usize,
    heat: Vec<f32>,
    /// いまの体の重心(場の座標、小数)。
    body_centre: (f32, f32),
    /// 加算の端を反対側へ折り返すか。原点 (0,0) 固定の体では false。
    toroidal: bool,
}

impl TouchEcho {
    /// すべてのセルが 0.0 の echo を作る。
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            heat: vec![MIN_HEAT; width * height],
            body_centre: (0.0, 0.0),
            toroidal: true,
        }
    }

    /// 折り返しを切る。壁で跳ね返る箱の体(粒子の体)用。`follow_body` を
    /// 呼ばない(原点 (0,0) のまま)のとセットで使う。
    pub fn without_wrap(&mut self) {
        self.toroidal = false;
    }

    /// 体の重心(場の座標)を受け取る。体が進むたびに呼ぶ。
    pub fn follow_body(&mut self, body_centre: (f32, f32)) {
        self.body_centre = body_centre;
    }

    /// 触れた位置に加算する。`Field::inject` と同じ山型・トーラス折り返しを共有する。
    ///
    /// `perturbation.at` は場の座標。いまの体の重心からの相対位置に直して記録する。
    /// `toroidal` が false のときは折り返さず、範囲外は捨てる
    pub fn touch(&mut self, perturbation: &Perturbation) {
        let on_body = CellPos {
            x: nearest_cell(perturbation.at.x as f32 - self.body_centre.0, self.width),
            y: nearest_cell(perturbation.at.y as f32 - self.body_centre.1, self.height),
        };
        accumulate_into_toroidal(
            &mut self.heat,
            self.width,
            self.height,
            &Perturbation {
                at: on_body,
                ..*perturbation
            },
            MIN_HEAT,
            MAX_HEAT,
            self.toroidal,
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
            body_centre: self.body_centre,
        }
    }
}

/// echo の読み取り専用ビュー。
#[derive(Debug, Clone, Copy)]
pub struct TouchEchoView<'a> {
    width: usize,
    height: usize,
    heat: &'a [f32],
    body_centre: (f32, f32),
}

impl TouchEchoView<'_> {
    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    /// 場の座標 `(x, y)` に見える光の強さ。範囲外の座標は 0.0 を返す。
    ///
    /// 体の重心からの相対位置(小数)を双線形補間して読む。重心が整数のときは
    /// 補間の重みが片側に寄り切るので、記録した値がそのまま返る。
    pub fn get(&self, x: usize, y: usize) -> f32 {
        sample_on_body(self.heat, self.width, self.height, self.body_centre, x, y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn the_glow_moves_with_the_body() {
        // Arrange: 体の右側を触る
        let mut echo = TouchEcho::new(32, 32);
        echo.touch(&Perturbation {
            at: CellPos { x: 8, y: 8 },
            radius: 3.0,
            amount: 0.5,
        });

        // Act: 体が9セル進む(O2u が1秒に進む距離)
        echo.follow_body((9.0, 0.0));

        // Assert: 光は体と一緒に9セル先へ移り、世界の元の座標には残らない
        let view = echo.view();
        assert!(
            (view.get(17, 8) - 0.5).abs() < 1e-5,
            "got {}",
            view.get(17, 8)
        );
        assert_eq!(view.get(8, 8), 0.0);
    }

    #[test]
    fn a_body_between_cells_is_read_smoothly_instead_of_snapping() {
        // Arrange
        let mut echo = TouchEcho::new(32, 32);
        echo.touch(&Perturbation {
            at: CellPos { x: 8, y: 8 },
            radius: 3.0,
            amount: 0.5,
        });

        // Act: 重心が半セルだけ進んだ状態
        echo.follow_body((0.5, 0.0));

        // Assert: 光の山は 8.5 にあるように見える。丸めて読んでいたら、
        // どちらか片方のセルに山ごと跳んでいたはず
        let view = echo.view();
        let (left, right) = (view.get(8, 8), view.get(9, 8));
        assert!((left - right).abs() < 1e-5, "left={left} right={right}");
        assert!(left < 0.5 && left > 0.3, "got {left}");
    }

    #[test]
    fn a_touch_on_a_moved_body_lights_where_it_landed() {
        // Arrange: 体がセルの間にいるときに触る
        let mut echo = TouchEcho::new(32, 32);
        echo.follow_body((3.3, 0.0));

        // Act
        echo.touch(&Perturbation {
            at: CellPos { x: 10, y: 8 },
            radius: 3.0,
            amount: 0.5,
        });

        // Assert: 記録時の丸め(0.3セル)ぶんだけ山から外れるが、触った位置が
        // ほぼ最も明るく光る
        let view = echo.view();
        let at_touch = view.get(10, 8);
        assert!(at_touch > 0.45, "got {at_touch}");
        assert!(at_touch > view.get(12, 8));
        assert!(at_touch > view.get(8, 8));
    }
}
