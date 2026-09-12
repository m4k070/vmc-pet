//! 体の場。値域を 0.0..=1.0 に正規化して保持する。

use super::animal::Pattern;
use super::perturbation::accumulate_into;
use super::Perturbation;

/// 場のセルが取りうる値の範囲。
const MIN_CELL_VALUE: f32 = 0.0;
const MAX_CELL_VALUE: f32 = 1.0;

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
}

#[cfg(test)]
mod tests {
    use super::*;

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
    use crate::body::perturbation::CellPos;

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
        assert_eq!(view.get(12, 8), 0.0, "outside the radius must stay untouched");
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
