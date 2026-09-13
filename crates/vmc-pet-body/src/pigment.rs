//! 体に付く色素。気分を「色」で見せるための、体の2つ目のチャンネル。
//!
//! docs/DESIGN.md「元気/待っている/がっかり」で、見た目の特徴が活発さの強弱
//! (移動の速さ・脈動)しか無いと、「待っている」と「元気」はどう動かしても見分け
//! られないと分かった。活発になる状態はどれも元気に寄っていくからだ。強弱とは独立
//! した軸として、ユーザーと相談して色を足すことにした。
//!
//! # 仮面ではなく、顔が赤らむのに近いもの
//!
//! 場とは別に「見せたい色」を描くと、表示が体の状態と食い違っても気づけない仮面に
//! なり、「体の内部状態=表現」というコンセプトに反する。そこで色素は体の状態として
//! 持ち、次の性質を持たせた:
//!
//! - **体があるところにしか溜まらない**。溜まる量はその場所の体の値に比例する
//! - **慣性がある**。時定数およそ20秒でゆっくり溜まり、ゆっくり抜ける。気分が
//!   変わっても瞬時には色が切り替わらない
//! - **体の動きには一切影響しない**。体 → 色素の一方向の結びつきだけなので、
//!   Lenia の崩壊の崖を増やさない(`Pet` のテストで固定)
//!
//! 別案として、色を Lenia の2つ目の生きた場にする(本来の多チャンネル Lenia)ことも
//! 検討したが、計算が約2倍になり(M5Stack には余裕が無い)、新しい崩壊の崖があり、
//! 出荷している生物は1チャンネル用のパラメータしか持たないため採らなかった
//! (ユーザーと相談して決定)。
//!
//! 体基準で覚えて体基準で読む(`body_frame` 参照)。生物は滑るように移動するので、
//! 場の座標で溜めると体が常に新しいセルに入り、色が溜まらない。

use alloc::vec;
use alloc::vec::Vec;

use crate::body_frame::{nearest_cell, sample_on_body};
use crate::FieldView;

/// 色素の値域。描画での色の混ぜ具合(0.0 = 体の色のまま、1.0 = 色素の色)に使う。
const MIN_LEVEL: f32 = 0.0;
const MAX_LEVEL: f32 = 1.0;

/// 1ステップごとに色素が入れ替わる割合。15 step/s で300ステップ、つまり時定数
/// およそ20秒。平衡では、その場所の色素は「体の値 × 刺激」になる。
const TURNOVER_PER_STEP: f32 = 1.0 / 300.0;

/// これ以下の体の値のセルには色素を溜めない(ほぼ体が無い場所)。
const NEGLIGIBLE_BODY_VALUE: f32 = 0.004;

/// 体に付く色素の場。内部の配列は体の重心を原点とした座標で並んでいる。
pub struct Pigment {
    width: usize,
    height: usize,
    level: Vec<f32>,
    body_centre: (f32, f32),
}

impl Pigment {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            level: vec![MIN_LEVEL; width * height],
            body_centre: (0.0, 0.0),
        }
    }

    /// 体の重心(場の座標)を受け取る。体が進むたびに呼ぶ。
    pub fn follow_body(&mut self, body_centre: (f32, f32)) {
        self.body_centre = body_centre;
    }

    /// 体が1ステップ進むごとに呼ぶ。どこでも少しずつ抜け、体があるところには
    /// `stimulus`(0.0..=1.0)の強さに応じて溜まる。
    pub fn step(&mut self, body: FieldView<'_>, stimulus: f32) {
        for level in &mut self.level {
            *level *= 1.0 - TURNOVER_PER_STEP;
        }
        let stimulus = stimulus.clamp(0.0, 1.0);
        if stimulus <= 0.0 {
            return;
        }
        for y in 0..self.height {
            for x in 0..self.width {
                let body_value = body.get(x, y);
                if body_value <= NEGLIGIBLE_BODY_VALUE {
                    continue;
                }
                let on_body_x = nearest_cell(x as f32 - self.body_centre.0, self.width);
                let on_body_y = nearest_cell(y as f32 - self.body_centre.1, self.height);
                let index = on_body_y * self.width + on_body_x;
                let inflow = TURNOVER_PER_STEP * stimulus * body_value;
                self.level[index] = (self.level[index] + inflow).min(MAX_LEVEL);
            }
        }
    }

    /// 読み取り専用ビューを返す。
    pub fn view(&self) -> PigmentView<'_> {
        PigmentView {
            width: self.width,
            height: self.height,
            level: &self.level,
            body_centre: self.body_centre,
        }
    }
}

/// 色素の読み取り専用ビュー。
#[derive(Debug, Clone, Copy)]
pub struct PigmentView<'a> {
    width: usize,
    height: usize,
    level: &'a [f32],
    body_centre: (f32, f32),
}

impl PigmentView<'_> {
    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    /// 場の座標 `(x, y)` に見える色素の濃さ(0.0..=1.0)。範囲外の座標は 0.0。
    pub fn get(&self, x: usize, y: usize) -> f32 {
        sample_on_body(self.level, self.width, self.height, self.body_centre, x, y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CellPos, Field, Perturbation};

    /// 中央に塊のある場。
    fn body_with_a_blob() -> Field {
        let mut field = Field::new(16, 16);
        field.inject(&Perturbation {
            at: CellPos { x: 8, y: 8 },
            radius: 3.0,
            amount: 1.0,
        });
        field
    }

    #[test]
    fn nothing_accumulates_without_a_stimulus() {
        // Arrange
        let body = body_with_a_blob();
        let mut pigment = Pigment::new(16, 16);

        // Act
        for _ in 0..600 {
            pigment.step(body.view(), 0.0);
        }

        // Assert
        assert_eq!(pigment.view().get(8, 8), 0.0);
    }

    #[test]
    fn pigment_builds_up_slowly_where_the_body_is() {
        // Arrange
        let body = body_with_a_blob();
        let mut pigment = Pigment::new(16, 16);

        // Act: 1秒ぶんと、40秒ぶん
        for _ in 0..15 {
            pigment.step(body.view(), 1.0);
        }
        let after_a_second = pigment.view().get(8, 8);
        for _ in 15..600 {
            pigment.step(body.view(), 1.0);
        }
        let after_forty_seconds = pigment.view().get(8, 8);

        // Assert: 瞬時には色づかず(慣性)、しばらくすると体の値に近い濃さになる。
        // 体が無いところには溜まらない
        assert!(
            after_a_second < 0.1,
            "must not flush instantly; got {after_a_second}"
        );
        let core = body.view().get(8, 8);
        assert!(
            after_forty_seconds > core * 0.8,
            "got {after_forty_seconds}, body {core}"
        );
        assert_eq!(pigment.view().get(0, 0), 0.0);
    }

    #[test]
    fn pigment_fades_slowly_once_the_stimulus_ends() {
        // Arrange: 十分に色づかせる
        let body = body_with_a_blob();
        let mut pigment = Pigment::new(16, 16);
        for _ in 0..900 {
            pigment.step(body.view(), 1.0);
        }
        let flushed = pigment.view().get(8, 8);

        // Act: 刺激が無くなってから1秒ぶんと、1分ぶん
        for _ in 0..15 {
            pigment.step(body.view(), 0.0);
        }
        let a_second_later = pigment.view().get(8, 8);
        for _ in 15..900 {
            pigment.step(body.view(), 0.0);
        }
        let a_minute_later = pigment.view().get(8, 8);

        // Assert
        assert!(a_second_later > flushed * 0.9, "must not vanish instantly");
        assert!(
            a_minute_later < flushed * 0.1,
            "must fade within about a minute"
        );
    }

    #[test]
    fn the_colour_moves_with_the_body() {
        // Arrange: 色づかせる
        let body = body_with_a_blob();
        let mut pigment = Pigment::new(16, 16);
        for _ in 0..900 {
            pigment.step(body.view(), 1.0);
        }
        let flushed = pigment.view().get(8, 8);

        // Act: 体が5セル進む
        pigment.follow_body((5.0, 0.0));

        // Assert: 色は体と一緒に移る
        assert_eq!(pigment.view().get(13, 8), flushed);
    }
}
