//! 場の値をドットマトリックスとして描く。
//!
//! 場(96x96)は表示(32x32)より高解像度なので、セルごとに平均プーリングして落とす。
//! プーリングは学習も表現も持たない純粋なダウンサンプルであり、
//! 「V に相当するエンコーダを実装しない」というコンセプトを壊さない。

use crate::body::FieldView;

/// ドット同士が接触しないよう、セル幅に対して空ける隙間の割合。
const DOT_GAP_RATIO: f32 = 0.18;

/// これ以下の値のセルは描画しない(ほぼ透明なドットを描く無駄を省く)。
const MIN_VISIBLE_VALUE: f32 = 0.004;

/// 円の縁をぼかす幅(ピクセル)。ジャギーを消すために使う。
const EDGE_FEATHER_PIXELS: f32 = 0.5;

const DOT_RED: f32 = 0.35;
const DOT_GREEN: f32 = 0.85;
const DOT_BLUE: f32 = 0.80;

const BYTES_PER_PIXEL: usize = 4;

/// サーフェス内でグリッドが占める矩形(論理ピクセル)。入力領域の算出にも使う。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridBounds {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// ドットマトリックスの配置と描画。
pub struct DotGrid {
    columns: usize,
    rows: usize,
}

impl DotGrid {
    pub fn new(columns: usize, rows: usize) -> Self {
        Self { columns, rows }
    }

    /// グリッドが占める矩形を返す。セルは正方形にし、サーフェス内で中央寄せする。
    /// サーフェスが小さすぎてセルが 1px 未満になる場合は幅 0 の矩形を返す。
    pub fn bounds(&self, width: u32, height: u32) -> GridBounds {
        let cell_size = (width as usize / self.columns).min(height as usize / self.rows);
        if cell_size == 0 {
            return GridBounds {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
            };
        }
        let grid_width = (cell_size * self.columns) as i32;
        let grid_height = (cell_size * self.rows) as i32;
        GridBounds {
            x: (width as i32 - grid_width) / 2,
            y: (height as i32 - grid_height) / 2,
            width: grid_width,
            height: grid_height,
        }
    }

    /// 場の値をドットとして描く。`canvas` は premultiplied ARGB8888。
    /// 呼び出し側が canvas をクリア済みであることを前提に、上書きで描く。
    pub fn draw(&self, field: FieldView<'_>, canvas: &mut [u8], width: u32, height: u32) {
        let bounds = self.bounds(width, height);
        if bounds.width == 0 {
            return;
        }
        let cell_size = bounds.width as f32 / self.columns as f32;
        let max_radius = cell_size * 0.5 * (1.0 - DOT_GAP_RATIO);

        for row in 0..self.rows {
            for column in 0..self.columns {
                let value = pool_cell(field, self.columns, self.rows, column, row);
                if value <= MIN_VISIBLE_VALUE {
                    continue;
                }
                // 面積が値に比例するよう半径は sqrt をとる
                let radius = max_radius * value.sqrt();
                let center_x = bounds.x as f32 + (column as f32 + 0.5) * cell_size;
                let center_y = bounds.y as f32 + (row as f32 + 0.5) * cell_size;
                draw_dot(canvas, width, height, center_x, center_y, radius, value);
            }
        }
    }
}

/// 表示セル1つ分に対応する場の矩形を平均する(ボックスフィルタ)。
/// 場と表示の解像度が割り切れない比でも破綻しないよう、区間を整数で切り出す。
fn pool_cell(field: FieldView<'_>, columns: usize, rows: usize, column: usize, row: usize) -> f32 {
    let x_start = column * field.width() / columns;
    let x_end = ((column + 1) * field.width() / columns).max(x_start + 1);
    let y_start = row * field.height() / rows;
    let y_end = ((row + 1) * field.height() / rows).max(y_start + 1);

    let mut total = 0.0;
    let mut count = 0.0;
    for y in y_start..y_end {
        for x in x_start..x_end {
            total += field.get(x, y);
            count += 1.0;
        }
    }
    total / count
}

/// 円を1つ描く。縁を1ピクセル分ぼかしてジャギーを消す。
fn draw_dot(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    center_x: f32,
    center_y: f32,
    radius: f32,
    value: f32,
) {
    let min_x = (center_x - radius - 1.0).floor().max(0.0) as usize;
    let min_y = (center_y - radius - 1.0).floor().max(0.0) as usize;
    let max_x = (center_x + radius + 1.0).ceil().clamp(0.0, width as f32) as usize;
    let max_y = (center_y + radius + 1.0).ceil().clamp(0.0, height as f32) as usize;

    for y in min_y..max_y {
        for x in min_x..max_x {
            let dx = x as f32 + 0.5 - center_x;
            let dy = y as f32 + 0.5 - center_y;
            let distance = (dx * dx + dy * dy).sqrt();
            let coverage = (radius + EDGE_FEATHER_PIXELS - distance).clamp(0.0, 1.0);
            if coverage <= 0.0 {
                continue;
            }
            let offset = (y * width as usize + x) * BYTES_PER_PIXEL;
            let pixel = premultiplied_argb8888(DOT_RED, DOT_GREEN, DOT_BLUE, value * coverage);
            canvas[offset..offset + BYTES_PER_PIXEL].copy_from_slice(&pixel);
        }
    }
}

/// 0.0〜1.0 の色とアルファを premultiplied ARGB8888 の1ピクセル分に変換する。
/// リトルエンディアン環境では u32 0xAARRGGBB がバイト列 [B, G, R, A] になる。
fn premultiplied_argb8888(red: f32, green: f32, blue: f32, alpha: f32) -> [u8; 4] {
    let to_byte = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
    [
        to_byte(blue * alpha),
        to_byte(green * alpha),
        to_byte(red * alpha),
        to_byte(alpha),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body::Field;

    #[test]
    fn premultiplied_argb8888_multiplies_each_channel_by_alpha() {
        // Arrange
        let alpha = 0.5;

        // Act
        let pixel = premultiplied_argb8888(1.0, 0.0, 1.0, alpha);

        // Assert
        assert_eq!(pixel, [128, 0, 128, 128]);
    }

    #[test]
    fn bounds_centers_a_square_grid_inside_the_surface() {
        // Arrange
        let grid = DotGrid::new(32, 32);

        // Act: 横長のサーフェスでは短辺がセルサイズを決める
        let bounds = grid.bounds(512, 384);

        // Assert
        assert_eq!(bounds.width, 384);
        assert_eq!(bounds.height, 384);
        assert_eq!(bounds.x, 64);
        assert_eq!(bounds.y, 0);
    }

    #[test]
    fn bounds_is_empty_when_the_surface_cannot_fit_one_pixel_per_cell() {
        // Arrange
        let grid = DotGrid::new(32, 32);

        // Act
        let bounds = grid.bounds(16, 16);

        // Assert
        assert_eq!(bounds.width, 0);
    }

    #[test]
    fn pool_cell_averages_the_matching_field_block() {
        // Arrange: 96x96 の場を 32x32 に落とすと 1セル = 3x3 ブロック
        let mut field = Field::new(96, 96);
        field.fill_with(|x, y| if x < 3 && y < 3 { 1.0 } else { 0.0 });

        // Act
        let first = pool_cell(field.view(), 32, 32, 0, 0);
        let second = pool_cell(field.view(), 32, 32, 1, 0);

        // Assert
        assert!((first - 1.0).abs() < f32::EPSILON);
        assert_eq!(second, 0.0);
    }

    #[test]
    fn draw_leaves_the_canvas_untouched_for_an_empty_field() {
        // Arrange
        let field = Field::new(96, 96);
        let grid = DotGrid::new(32, 32);
        let mut canvas = vec![0u8; 384 * 384 * BYTES_PER_PIXEL];

        // Act
        grid.draw(field.view(), &mut canvas, 384, 384);

        // Assert
        assert!(canvas.iter().all(|&byte| byte == 0));
    }

    #[test]
    fn draw_keeps_dots_inside_their_own_cell() {
        // Arrange: 1セルだけ最大値にする
        let mut field = Field::new(96, 96);
        field.fill_with(|x, y| if x < 3 && y < 3 { 1.0 } else { 0.0 });
        let grid = DotGrid::new(32, 32);
        let surface_size = 384;
        let mut canvas = vec![0u8; surface_size * surface_size * BYTES_PER_PIXEL];

        // Act
        grid.draw(field.view(), &mut canvas, surface_size as u32, surface_size as u32);

        // Assert: セル境界(x=12)より右には染み出さない
        let cell_size = surface_size / 32;
        let row_offset = (cell_size / 2) * surface_size;
        let outside = (row_offset + cell_size) * BYTES_PER_PIXEL;
        assert_eq!(canvas[outside + 3], 0, "dot must not bleed into the next cell");
        let inside = (row_offset + cell_size / 2) * BYTES_PER_PIXEL;
        assert_ne!(canvas[inside + 3], 0, "dot must cover its own cell center");
    }
}
