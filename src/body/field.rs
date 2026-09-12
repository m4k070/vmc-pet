//! 体の場。値域を 0.0..=1.0 に正規化して保持する。

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

    /// 全セルを `next` の返り値で置き換える。
    /// 値域の不変条件はここで守るため、`next` 側は clamp しなくてよい。
    pub fn fill_with(&mut self, next: impl Fn(usize, usize) -> f32) {
        for y in 0..self.height {
            for x in 0..self.width {
                self.cells[y * self.width + x] =
                    next(x, y).clamp(MIN_CELL_VALUE, MAX_CELL_VALUE);
            }
        }
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
    fn fill_with_clamps_values_into_the_normalized_range() {
        // Arrange
        let mut field = Field::new(2, 2);

        // Act: 値域外を返す関数を渡す
        field.fill_with(|x, _y| if x == 0 { -5.0 } else { 5.0 });

        // Assert
        let view = field.view();
        assert_eq!(view.get(0, 0), MIN_CELL_VALUE);
        assert_eq!(view.get(1, 0), MAX_CELL_VALUE);
    }

    #[test]
    fn get_returns_zero_outside_the_field() {
        // Arrange
        let mut field = Field::new(2, 2);
        field.fill_with(|_x, _y| 1.0);

        // Act
        let view = field.view();

        // Assert
        assert_eq!(view.get(2, 0), 0.0);
        assert_eq!(view.get(0, 2), 0.0);
    }

    #[test]
    fn fill_with_receives_every_coordinate_once() {
        // Arrange
        let mut field = Field::new(3, 2);

        // Act: 座標を値に符号化して全セルに行き渡ることを確かめる
        field.fill_with(|x, y| (y * 3 + x) as f32 / 10.0);

        // Assert
        let view = field.view();
        assert_eq!(view.get(0, 0), 0.0);
        assert!((view.get(2, 1) - 0.5).abs() < f32::EPSILON);
    }
}
