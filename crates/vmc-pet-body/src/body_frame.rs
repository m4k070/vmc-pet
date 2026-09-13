//! 体の重心を原点とした座標(体基準)と、場の座標の間の変換。
//!
//! 生物は場の上を滑るように移動し(O2u は約9セル/秒)、カメラはそれを追う。
//! 「体に付いているもの」を場の座標で覚えると、体が進むたびに置き去りになる。
//! そこで echo(触れた跡の光)と色素(体の色づき)は、体基準で覚えて体基準で読む。
//!
//! 読み出しは双線形補間する。重心を整数セルに丸めて読むと、重心が1セル進むたびに
//! 最大±0.5セル跳ね、以前カメラの原点を丸めたとき体全体で見えた揺れ(camera.rs)を
//! 再現してしまうため。記録は1回きりなので、そこでの丸め(最大0.5セル)は揺れにならない。
//!
//! 慣れ(habituation)も体基準だが、描画に使わずセル単位で足りるので、整数の
//! 重心で自前に変換している。

use crate::math::{floorf, rem_euclidf};

/// トーラス上の小数の位置に最も近いセル。
pub(crate) fn nearest_cell(position: f32, size: usize) -> usize {
    floorf(rem_euclidf(position + 0.5, size as f32)) as usize % size
}

/// 体基準で並んだ値 `values` を、場の座標 `(x, y)` から見た値として読む。
///
/// 体の重心からの相対位置(小数)を双線形補間する。重心が整数のときは補間の重みが
/// 片側に寄り切るので、記録した値がそのまま返る。範囲外の座標は 0.0。
pub(crate) fn sample_on_body(
    values: &[f32],
    width: usize,
    height: usize,
    body_centre: (f32, f32),
    x: usize,
    y: usize,
) -> f32 {
    if x >= width || y >= height {
        return 0.0;
    }
    let on_body_x = rem_euclidf(x as f32 - body_centre.0, width as f32);
    let on_body_y = rem_euclidf(y as f32 - body_centre.1, height as f32);
    let (left, right, toward_right) = neighbours(on_body_x, width);
    let (top, bottom, toward_bottom) = neighbours(on_body_y, height);

    let cell = |column: usize, row: usize| values[row * width + column];
    let upper = cell(left, top) * (1.0 - toward_right) + cell(right, top) * toward_right;
    let lower = cell(left, bottom) * (1.0 - toward_right) + cell(right, bottom) * toward_right;
    upper * (1.0 - toward_bottom) + lower * toward_bottom
}

/// `0.0..size` の小数の位置を挟む2つのセル(トーラスで折り返す)と、
/// 後ろのセルへの寄り具合(0.0..1.0)。
fn neighbours(position: f32, size: usize) -> (usize, usize, f32) {
    let before = floorf(position);
    let first = before as usize % size;
    (first, (first + 1) % size, position - before)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_position_left_of_the_origin_wraps_to_the_far_side() {
        // Arrange / Act / Assert: -0.6 は -1 に近く、トーラスでは右端のセル
        assert_eq!(nearest_cell(-0.6, 32), 31);
        assert_eq!(nearest_cell(-0.4, 32), 0);
    }

    #[test]
    fn an_integer_body_centre_reads_the_stored_value_exactly() {
        // Arrange
        let (width, row, column) = (4, 1, 2);
        let mut values = [0.0f32; 16];
        values[row * width + column] = 0.75;

        // Act: 重心 (1, 0) の体を、場の (3, 1) から見る = 体基準の (2, 1)
        let read = sample_on_body(&values, 4, 4, (1.0, 0.0), 3, 1);

        // Assert
        assert_eq!(read, 0.75);
    }
}
