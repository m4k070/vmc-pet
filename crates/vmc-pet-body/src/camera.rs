//! 生物が場の端で分断されて見えないよう、表示の原点をずらす。
//!
//! 場はトーラスなので、平行移動は力学に対する厳密な対称性である。
//! したがって表示側で原点をずらしても、体の状態と表示の間にずれは生じない。
//! 場そのものは書き換えないため、体は純粋なまま保たれる。
//!
//! PC版(`src/render/dot_grid.rs`)・M5Stack版(`m5stack-cores3/src/bin/main.rs`)
//! のどちらも「表示の原点を重心へ寄せる」という同じ理由でこれを使うため、
//! ここに一本化してある(std/no_std どちらでも使える)。

use crate::FieldView;

/// 表示の原点。画面左上のドットに対応する場の座標を、小数のまま保持する。
///
/// 整数に丸めてしまうと、生物の実位置と表示位置の差が ±0.5 セル(=ドット1個分)の
/// のこぎり波になって現れ、毎秒数回の揺れとして見える。生物自身の動きは滑らかで
/// (等速直線への残差 RMS 0.017 セル)、揺れはすべて丸めが作っていた。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    x: f32,
    y: f32,
}

impl Camera {
    pub fn new() -> Self {
        Self { x: 0.0, y: 0.0 }
    }

    /// 場の重心が表示の中央に来るよう原点を更新する。
    /// 場が空の場合は直前の原点を保つ。
    pub fn follow(&mut self, field: FieldView<'_>, columns: usize, rows: usize) {
        let Some((centre_x, centre_y)) = field.toroidal_centroid() else {
            return;
        };
        self.x = wrap_origin(centre_x, columns, field.width());
        self.y = wrap_origin(centre_y, rows, field.height());
    }

    /// 画面左上に対応する場の座標。
    /// 整数部がどのセルから読むかを、小数部がドットをずらす量を決める。
    pub fn origin(&self) -> (f32, f32) {
        (self.x, self.y)
    }
}

impl Default for Camera {
    fn default() -> Self {
        Self::new()
    }
}

/// 重心を表示中央に置くための原点を、場の範囲へ折り返して求める。
fn wrap_origin(centre: f32, visible: usize, field_size: usize) -> f32 {
    crate::math::rem_euclidf(centre - visible as f32 / 2.0, field_size as f32)
}

#[cfg(test)]
mod tests {
    use crate::Field;

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
    fn follow_puts_a_straddling_block_back_in_the_middle() {
        // Arrange: 継ぎ目をまたぐ塊
        let field = field_with_block(32, 30, 30, 4);
        let mut camera = Camera::new();

        // Act
        camera.follow(field.view(), 32, 32);

        // Assert: 原点から見て塊が画面中央付近に来る
        let (origin_x, origin_y) = camera.origin();
        let centre_on_screen = crate::math::rem_euclidf(31.5 - origin_x, 32.0);
        assert!(
            (centre_on_screen - 16.0).abs() <= 0.5,
            "block sits at {centre_on_screen} instead of the middle"
        );
        assert_eq!(
            origin_x, origin_y,
            "a symmetric block must give a symmetric origin"
        );
    }

    #[test]
    fn follow_keeps_the_previous_origin_for_an_empty_field() {
        // Arrange
        let populated = field_with_block(32, 30, 30, 4);
        let mut camera = Camera::new();
        camera.follow(populated.view(), 32, 32);
        let before = camera.origin();

        // Act
        let empty = Field::new(32, 32);
        camera.follow(empty.view(), 32, 32);

        // Assert
        assert_eq!(camera.origin(), before);
    }

    #[test]
    fn origin_keeps_the_sub_cell_part_of_the_centroid() {
        // Arrange: 重心が 11.5 に来る塊。原点は 11.5 - 16 = -4.5 → 折り返して 27.5
        let field = field_with_block(32, 10, 10, 4);
        let mut camera = Camera::new();

        // Act
        camera.follow(field.view(), 32, 32);

        // Assert: 整数に丸められていないこと。丸めると ±0.5 セルの揺れになる
        let (x, _y) = camera.origin();
        assert!((x - 27.5).abs() < 0.05, "got {x}");
        assert!(x.fract() != 0.0, "the sub-cell part must survive");
    }
}
