//! 体の場。値域を 0.0..=1.0 に正規化して保持する。

use core::f32::consts::TAU;

use alloc::vec;
use alloc::vec::Vec;

use super::animal::Pattern;
use super::perturbation::accumulate_into;
use super::Perturbation;

/// 場のセルが取りうる値の範囲。
const MIN_CELL_VALUE: f32 = 0.0;
const MAX_CELL_VALUE: f32 = 1.0;

/// これ以下の総量しかない場では重心を定めない。
const NEGLIGIBLE_MASS: f32 = 1e-6;

/// 体の場。セルは行優先で並ぶ。
pub struct Field {
    width: usize,
    height: usize,
    cells: Vec<f32>,
}

impl Field {
    /// すべてのセルが 0.0 の場を作る。
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            cells: vec![MIN_CELL_VALUE; width * height],
        }
    }

    /// 全セルを `next` の返り値で置き換える。`next` は現在の値を受け取る。
    /// 値域の不変条件はここで守るため、呼び出し側は clamp しなくてよい。
    pub fn map(&mut self, next: impl Fn(usize, usize, f32) -> f32) {
        for y in 0..self.height {
            for x in 0..self.width {
                let index = y * self.width + x;
                self.cells[index] =
                    next(x, y, self.cells[index]).clamp(MIN_CELL_VALUE, MAX_CELL_VALUE);
            }
        }
    }

    /// 摂動を場に注入する。場はトーラスなので、半径が端を越えた分は反対側へ回り込む。
    pub fn inject(&mut self, perturbation: &Perturbation) {
        accumulate_into(
            &mut self.cells,
            self.width,
            self.height,
            perturbation,
            MIN_CELL_VALUE,
            MAX_CELL_VALUE,
        );
    }

    /// 場を空にする。
    pub fn clear(&mut self) {
        self.cells.fill(MIN_CELL_VALUE);
    }

    /// 場の総量。体が生きているかの目安になる。
    pub fn mass(&self) -> f32 {
        self.cells.iter().sum()
    }

    /// 生物のパターンを場の中央に配置する。既存の値は上書きする。
    pub fn place_centered(&mut self, pattern: &Pattern) {
        let left = self.width.saturating_sub(pattern.width()) / 2;
        let top = self.height.saturating_sub(pattern.height()) / 2;
        self.map(|x, y, value| {
            if x < left || y < top {
                return value;
            }
            let inside = x - left < pattern.width() && y - top < pattern.height();
            if !inside {
                return value;
            }
            pattern.get(x - left, y - top)
        });
    }

    /// 読み取り専用ビューを返す。内部の Vec は外へ出さない。
    pub fn view(&self) -> FieldView<'_> {
        FieldView {
            width: self.width,
            height: self.height,
            cells: &self.cells,
        }
    }
}

/// 場の読み取り専用ビュー。
/// IF層の向こう側から場を直接書き換える経路を、型のレベルで塞ぐためにある。
#[derive(Debug, Clone, Copy)]
pub struct FieldView<'a> {
    width: usize,
    height: usize,
    cells: &'a [f32],
}

impl FieldView<'_> {
    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    /// 範囲外の座標は 0.0 を返す(場の外側には何もない)。
    pub fn get(&self, x: usize, y: usize) -> f32 {
        if x >= self.width || y >= self.height {
            return MIN_CELL_VALUE;
        }
        self.cells[y * self.width + x]
    }

    /// トーラス上の重心を円周平均で求める。場が空(総量がほぼ0)なら `None`。
    ///
    /// 生物が継ぎ目をまたいでいるとき、単純な算術平均は場の反対側を指してしまう。
    /// 座標を角度に写してから平均することで、継ぎ目をまたいでも正しい重心が得られる。
    /// PC版のカメラ追従(表示の原点をずらす)と、体の形の観測(コントローラの
    /// 判断材料)の両方が同じ計算を必要とするため、ここに一本化してある。
    pub fn toroidal_centroid(&self) -> Option<(f32, f32)> {
        let width = self.width as f32;
        let height = self.height as f32;
        let (mut x_cos, mut x_sin) = (0.0, 0.0);
        let (mut y_cos, mut y_sin) = (0.0, 0.0);
        let mut mass = 0.0;

        for y in 0..self.height {
            for x in 0..self.width {
                let value = self.get(x, y);
                if value <= 0.0 {
                    continue;
                }
                mass += value;
                let x_angle = x as f32 / width * TAU;
                let y_angle = y as f32 / height * TAU;
                x_cos += value * crate::math::cosf(x_angle);
                x_sin += value * crate::math::sinf(x_angle);
                y_cos += value * crate::math::cosf(y_angle);
                y_sin += value * crate::math::sinf(y_angle);
            }
        }

        if mass <= NEGLIGIBLE_MASS {
            return None;
        }
        Some((
            crate::math::rem_euclidf(crate::math::atan2f(x_sin, x_cos), TAU) / TAU * width,
            crate::math::rem_euclidf(crate::math::atan2f(y_sin, y_cos), TAU) / TAU * height,
        ))
    }
}

/// トーラス上の符号付き最短オフセット。`size/2` を超えたら反対側から測り直す。
/// 「AからBへの最短距離(向き付き)」を求める場面で共通して使う
/// (`controller.rs` の歪み計算、`fitness.rs` の重心の移動量など)。
pub(crate) fn toroidal_signed_offset(raw: f32, size: f32) -> f32 {
    let wrapped = crate::math::rem_euclidf(raw, size);
    if wrapped > size / 2.0 {
        wrapped - size
    } else {
        wrapped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 指定した位置から `extent` 四方だけを 1.0 にした場を作る。トーラス上で折り返す。
    fn field_with_block(size: usize, left: usize, top: usize, extent: usize) -> Field {
        let mut field = Field::new(size, size);
        field.map(|x, y, _value| {
            let inside_x = (x + size - left) % size < extent;
            let inside_y = (y + size - top) % size < extent;
            if inside_x && inside_y {
                1.0
            } else {
                0.0
            }
        });
        field
    }

    #[test]
    fn toroidal_centroid_matches_the_arithmetic_mean_away_from_the_seam() {
        // Arrange: 継ぎ目から離れた位置に 4x4 の塊を置く
        let field = field_with_block(32, 10, 10, 4);

        // Act
        let (x, y) = field.view().toroidal_centroid().unwrap();

        // Assert: 10..14 の中心は 11.5
        assert!((x - 11.5).abs() < 0.1, "got {x}");
        assert!((y - 11.5).abs() < 0.1, "got {y}");
    }

    #[test]
    fn toroidal_centroid_handles_a_block_straddling_the_seam() {
        // Arrange: x=30,31,0,1 にまたがる塊。算術平均なら 15.5 という誤った答えになる
        let field = field_with_block(32, 30, 10, 4);

        // Act
        let (x, _y) = field.view().toroidal_centroid().unwrap();

        // Assert: 正しい重心は 31.5
        let error = (x - 31.5).abs().min(32.0 - (x - 31.5).abs());
        assert!(error < 0.1, "got {x}");
    }

    #[test]
    fn toroidal_signed_offset_takes_the_short_way_around() {
        // Arrange / Act / Assert: 32幅の場で 30 → 2 は、素直に引けば -28 だが、
        // 反対回り(+4)の方が短いのでそちらを返す
        assert!((toroidal_signed_offset(2.0 - 30.0, 32.0) - 4.0).abs() < 1e-5);
        // 近い場合はそのまま
        assert!((toroidal_signed_offset(5.0 - 3.0, 32.0) - 2.0).abs() < 1e-5);
    }

    #[test]
    fn toroidal_centroid_is_undefined_for_an_empty_field() {
        // Arrange
        let field = Field::new(32, 32);

        // Act / Assert
        assert!(field.view().toroidal_centroid().is_none());
    }

    #[test]
    fn map_clamps_values_into_the_normalized_range() {
        // Arrange
        let mut field = Field::new(2, 2);

        // Act: 値域外を返す関数を渡す
        field.map(|x, _y, _value| if x == 0 { -5.0 } else { 5.0 });

        // Assert
        let view = field.view();
        assert_eq!(view.get(0, 0), MIN_CELL_VALUE);
        assert_eq!(view.get(1, 0), MAX_CELL_VALUE);
    }

    #[test]
    fn map_receives_the_current_value() {
        // Arrange
        let mut field = Field::new(2, 1);
        field.map(|x, _y, _value| x as f32 * 0.5);

        // Act
        field.map(|_x, _y, value| value + 0.25);

        // Assert
        let view = field.view();
        assert_eq!(view.get(0, 0), 0.25);
        assert_eq!(view.get(1, 0), 0.75);
    }

    #[test]
    fn get_returns_zero_outside_the_field() {
        // Arrange
        let mut field = Field::new(2, 2);
        field.map(|_x, _y, _value| 1.0);

        // Act
        let view = field.view();

        // Assert
        assert_eq!(view.get(2, 0), 0.0);
        assert_eq!(view.get(0, 2), 0.0);
    }
}

#[cfg(test)]
mod injection_tests {
    use super::*;
    use crate::perturbation::CellPos;

    #[test]
    fn inject_raises_the_centre_most() {
        // Arrange
        let mut field = Field::new(16, 16);

        // Act
        field.inject(&Perturbation {
            at: CellPos { x: 8, y: 8 },
            radius: 3.0,
            amount: 0.5,
        });

        // Assert
        let view = field.view();
        assert!((view.get(8, 8) - 0.5).abs() < 1e-5);
        assert!(view.get(9, 8) < view.get(8, 8));
        assert_eq!(
            view.get(12, 8),
            0.0,
            "outside the radius must stay untouched"
        );
    }

    #[test]
    fn inject_wraps_around_the_torus() {
        // Arrange: 場の端に注入する
        let mut field = Field::new(16, 16);

        // Act
        field.inject(&Perturbation {
            at: CellPos { x: 0, y: 0 },
            radius: 3.0,
            amount: 0.5,
        });

        // Assert: 反対側の端にも回り込んでいる
        let view = field.view();
        assert!(view.get(15, 0) > 0.0, "the blob must wrap to the far edge");
        assert!(view.get(0, 15) > 0.0);
    }

    #[test]
    fn inject_keeps_values_within_the_normalized_range() {
        // Arrange: すでに満杯の場へさらに注入する
        let mut field = Field::new(16, 16);
        field.map(|_x, _y, _value| 1.0);

        // Act
        field.inject(&Perturbation {
            at: CellPos { x: 8, y: 8 },
            radius: 3.0,
            amount: 5.0,
        });

        // Assert
        assert_eq!(field.view().get(8, 8), MAX_CELL_VALUE);
    }

    #[test]
    fn a_negative_amount_carves_the_field_out() {
        // Arrange
        let mut field = Field::new(16, 16);
        field.map(|_x, _y, _value| 1.0);

        // Act
        field.inject(&Perturbation {
            at: CellPos { x: 8, y: 8 },
            radius: 3.0,
            amount: -0.5,
        });

        // Assert
        assert!((field.view().get(8, 8) - 0.5).abs() < 1e-5);
    }
}
