//! 場の値をドットマトリックスとして描く。
//!
//! 場(96x96)は表示(32x32)より高解像度なので、セルごとに平均プーリングして落とす。
//! プーリングは学習も表現も持たない純粋なダウンサンプルであり、
//! 「V に相当するエンコーダを実装しない」というコンセプトを壊さない。
//!
//! 体の場(body)と入力の echo(touch_echo)は別データとして受け取り、ここで初めて
//! 1つの絵に合成する。合成は見た目だけの都合であり、どちらの値も書き換えない。

use crate::render::TouchEchoView;
use vmc_pet_body::appearance::{appearance_of, DOT_GAP_RATIO, MIN_VISIBLE_VALUE};
use vmc_pet_body::{CellPos, FieldView, PigmentView};

/// 円の縁をぼかす幅(ピクセル)。ジャギーを消すために使う。
const EDGE_FEATHER_PIXELS: f32 = 0.5;

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

    /// 画面上の位置に対応する場のセルを返す。グリッドの外なら `None`。
    ///
    /// `draw` が行う座標変換の逆写像。表示原点の小数部だけずれている点も含めて
    /// 逆に辿るため、見えているドットと押した位置が一致する。
    pub fn cell_at(
        &self,
        position: (f64, f64),
        origin: (f32, f32),
        field_size: (usize, usize),
        width: u32,
        height: u32,
    ) -> Option<CellPos> {
        let bounds = self.bounds(width, height);
        if bounds.width == 0 {
            return None;
        }
        let (x, y) = (position.0 as f32, position.1 as f32);
        let inside = x >= bounds.x as f32
            && x < (bounds.x + bounds.width) as f32
            && y >= bounds.y as f32
            && y < (bounds.y + bounds.height) as f32;
        if !inside {
            return None;
        }

        let cell_size = bounds.width as f32 / self.columns as f32;
        let cell_origin = (origin.0.floor() as i32, origin.1.floor() as i32);
        let shift = (origin.0 - origin.0.floor(), origin.1 - origin.1.floor());

        let column = ((x - bounds.x as f32) / cell_size + shift.0).floor() as i32;
        let row = ((y - bounds.y as f32) / cell_size + shift.1).floor() as i32;

        let (field_width, field_height) = (field_size.0 as i32, field_size.1 as i32);
        let field_x =
            (column * field_width / self.columns as i32 + cell_origin.0).rem_euclid(field_width);
        let field_y =
            (row * field_height / self.rows as i32 + cell_origin.1).rem_euclid(field_height);

        Some(CellPos {
            x: field_x as usize,
            y: field_y as usize,
        })
    }

    /// 色素なしで描く。体と echo だけを見るテストのための便宜関数。
    #[cfg(test)]
    pub fn draw(
        &self,
        field: FieldView<'_>,
        echo: TouchEchoView<'_>,
        origin: (f32, f32),
        canvas: &mut [u8],
        width: u32,
        height: u32,
    ) {
        self.draw_with_pigment(field, echo, None, origin, canvas, width, height);
    }

    /// 体の場・入力の echo・体に付く色素を1つのドットマトリックスとして描く。
    /// `canvas` は premultiplied ARGB8888。呼び出し側が canvas をクリア済みであることを
    /// 前提に、上書きで描く。
    ///
    /// `origin` は画面左上に対応する場の座標。整数部がどのセルから読むかを、
    /// 小数部がドットをずらす量を決める。値はリサンプルせずそのまま使い、
    /// ドットの位置だけをサブピクセルでずらすため、にじみは生じない。
    /// 場も echo も色素も同じ座標系・同じトーラスとして扱うため、同じ origin を使う。
    ///
    /// 1セルの見え方は、体と echo の値のうち大きい方でドットの大きさ(=見えるかどうか)
    /// を決める。色は、まず色素の濃さに応じて体の色(ティール)を珊瑚色へ寄せ、
    /// その上に echo が占める割合で echo の色を混ぜる。触れていない場所は体の色、
    /// 体が無く echo だけがある場所(空き地を撫でたときなど)は純粋に echo の色になる。
    /// 色素はドットの大きさを変えないので、体が無いところの色素は描かれない。
    #[allow(clippy::too_many_arguments)]
    pub fn draw_with_pigment(
        &self,
        field: FieldView<'_>,
        echo: TouchEchoView<'_>,
        pigment: Option<PigmentView<'_>>,
        origin: (f32, f32),
        canvas: &mut [u8],
        width: u32,
        height: u32,
    ) {
        let bounds = self.bounds(width, height);
        if bounds.width == 0 {
            return;
        }
        let cell_size = bounds.width as f32 / self.columns as f32;
        let max_radius = cell_size * 0.5 * (1.0 - DOT_GAP_RATIO);

        let cell_origin = (origin.0.floor() as i32, origin.1.floor() as i32);
        let shift = (origin.0 - origin.0.floor(), origin.1 - origin.1.floor());

        // ずらした分だけ端に隙間ができるため、上下左右へ1列ずつ余分に描く
        for row in -1..=self.rows as i32 {
            for column in -1..=self.columns as i32 {
                let body_value = pool_cell(
                    |x, y| field.get(x, y),
                    (field.width(), field.height()),
                    cell_origin,
                    self.columns,
                    self.rows,
                    column,
                    row,
                );
                let echo_value = pool_cell(
                    |x, y| echo.get(x, y),
                    (echo.width(), echo.height()),
                    cell_origin,
                    self.columns,
                    self.rows,
                    column,
                    row,
                );
                let tint = pigment.map_or(0.0, |pigment| {
                    pool_cell(
                        |x, y| pigment.get(x, y),
                        (pigment.width(), pigment.height()),
                        cell_origin,
                        self.columns,
                        self.rows,
                        column,
                        row,
                    )
                });
                // 1セルの見え方(大きさ・色)は M5Stack版と共有する決まりに従う
                // (`vmc_pet_body::appearance`)。ここが持つのはドットの配置だけ。
                let Some(appearance) = appearance_of(body_value, echo_value, tint) else {
                    continue;
                };
                let color = appearance.color;
                let dot = Dot {
                    center_x: bounds.x as f32 + (column as f32 + 0.5 - shift.0) * cell_size,
                    center_y: bounds.y as f32 + (row as f32 + 0.5 - shift.1) * cell_size,
                    radius: max_radius * appearance.size,
                    value: appearance.opacity,
                    color: (color.red, color.green, color.blue),
                };
                draw_dot(canvas, width, height, bounds, dot);
            }
        }
    }
}

impl DotGrid {
    /// 【実験】多チャンネルの場を描く(`--preview-multichannel`)。チャンネルを赤・緑・青に割り当てる。
    ///
    /// ドットの大きさと不透明度は、体の場と同じ決まり(全チャンネルのうち一番大きい値の平方根と、
    /// その値そのもの)に従う。色は、各チャンネルの値を一番大きい値で割った割合。2チャンネルなら
    /// 赤と緑が重なったところが黄色になる。echo と色素は描かない。
    pub fn draw_channels(
        &self,
        channels: &[Vec<f32>],
        field_size: usize,
        origin: (f32, f32),
        canvas: &mut [u8],
        width: u32,
        height: u32,
    ) {
        let bounds = self.bounds(width, height);
        if bounds.width == 0 {
            return;
        }
        let cell_size = bounds.width as f32 / self.columns as f32;
        let max_radius = cell_size * 0.5 * (1.0 - DOT_GAP_RATIO);
        let cell_origin = (origin.0.floor() as i32, origin.1.floor() as i32);
        let shift = (origin.0 - origin.0.floor(), origin.1 - origin.1.floor());

        for row in -1..=self.rows as i32 {
            for column in -1..=self.columns as i32 {
                let pooled: Vec<f32> = channels
                    .iter()
                    .map(|values| {
                        pool_cell(
                            |x, y| values[y * field_size + x],
                            (field_size, field_size),
                            cell_origin,
                            self.columns,
                            self.rows,
                            column,
                            row,
                        )
                    })
                    .collect();
                let visibility = pooled.iter().copied().fold(0.0, f32::max);
                if visibility <= MIN_VISIBLE_VALUE {
                    continue;
                }
                let share = |channel: usize| pooled.get(channel).map_or(0.0, |v| v / visibility);
                let dot = Dot {
                    center_x: bounds.x as f32 + (column as f32 + 0.5 - shift.0) * cell_size,
                    center_y: bounds.y as f32 + (row as f32 + 0.5 - shift.1) * cell_size,
                    radius: max_radius * visibility.sqrt(),
                    value: visibility.min(1.0),
                    color: (share(0), share(1), share(2)),
                };
                draw_dot(canvas, width, height, bounds, dot);
            }
        }
    }
}

/// 表示セル1つ分に対応する矩形を平均する(ボックスフィルタ)。
/// 場・echo のどちらでも使えるよう、値の読み出しをクロージャで受け取る。
/// 解像度が割り切れない比でも破綻しないよう、区間を整数で切り出す。
/// 表示セルの添字は負にもなりうる(端の余分な1列)ため、符号付きで扱う。
fn pool_cell(
    get: impl Fn(usize, usize) -> f32,
    source_size: (usize, usize),
    cell_origin: (i32, i32),
    columns: usize,
    rows: usize,
    column: i32,
    row: i32,
) -> f32 {
    let (source_width, source_height) = (source_size.0 as i32, source_size.1 as i32);
    let x_start = column * source_width / columns as i32;
    let x_end = ((column + 1) * source_width / columns as i32).max(x_start + 1);
    let y_start = row * source_height / rows as i32;
    let y_end = ((row + 1) * source_height / rows as i32).max(y_start + 1);

    let mut total = 0.0;
    let mut count = 0.0;
    for y in y_start..y_end {
        for x in x_start..x_end {
            // 表示原点を足したうえでトーラス上に折り返す
            let source_x = (x + cell_origin.0).rem_euclid(source_width);
            let source_y = (y + cell_origin.1).rem_euclid(source_height);
            total += get(source_x as usize, source_y as usize);
            count += 1.0;
        }
    }
    total / count
}

/// 描画する1つのドット。位置はサブピクセル精度で持つ。
#[derive(Debug, Clone, Copy)]
struct Dot {
    center_x: f32,
    center_y: f32,
    radius: f32,
    value: f32,
    color: (f32, f32, f32),
}

/// 円を1つ描く。縁を1ピクセル分ぼかしてジャギーを消す。
/// 端の余分な1列がグリッドの外へはみ出さないよう、`bounds` で切り取る。
fn draw_dot(canvas: &mut [u8], width: u32, height: u32, bounds: GridBounds, dot: Dot) {
    let clip_left = bounds.x.max(0) as f32;
    let clip_top = bounds.y.max(0) as f32;
    let clip_right = ((bounds.x + bounds.width) as f32).min(width as f32);
    let clip_bottom = ((bounds.y + bounds.height) as f32).min(height as f32);

    let min_x = (dot.center_x - dot.radius - 1.0).floor().max(clip_left) as usize;
    let min_y = (dot.center_y - dot.radius - 1.0).floor().max(clip_top) as usize;
    let max_x = (dot.center_x + dot.radius + 1.0)
        .ceil()
        .clamp(clip_left, clip_right) as usize;
    let max_y = (dot.center_y + dot.radius + 1.0)
        .ceil()
        .clamp(clip_top, clip_bottom) as usize;

    for y in min_y..max_y {
        for x in min_x..max_x {
            let dx = x as f32 + 0.5 - dot.center_x;
            let dy = y as f32 + 0.5 - dot.center_y;
            let distance = (dx * dx + dy * dy).sqrt();
            let coverage = (dot.radius + EDGE_FEATHER_PIXELS - distance).clamp(0.0, 1.0);
            if coverage <= 0.0 {
                continue;
            }
            let offset = (y * width as usize + x) * BYTES_PER_PIXEL;
            let pixel =
                premultiplied_argb8888(dot.color.0, dot.color.1, dot.color.2, dot.value * coverage);
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
    use vmc_pet_body::{Field, TouchEcho};

    /// echo なしの場合の便宜関数。ほとんどのテストは体だけを見ている。
    fn draw_body_only(
        grid: &DotGrid,
        field: FieldView<'_>,
        origin: (f32, f32),
        canvas: &mut [u8],
        width: u32,
        height: u32,
    ) {
        let empty_echo = TouchEcho::new(field.width(), field.height());
        grid.draw(field, empty_echo.view(), origin, canvas, width, height);
    }

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
    fn pool_cell_averages_the_matching_block() {
        // Arrange: 96x96 を 32x32 に落とすと 1セル = 3x3 ブロック
        let mut field = Field::new(96, 96);
        field.map(|x, y, _value| if x < 3 && y < 3 { 1.0 } else { 0.0 });
        let view = field.view();

        // Act
        let first = pool_cell(
            |x, y| view.get(x, y),
            (view.width(), view.height()),
            (0, 0),
            32,
            32,
            0,
            0,
        );
        let second = pool_cell(
            |x, y| view.get(x, y),
            (view.width(), view.height()),
            (0, 0),
            32,
            32,
            1,
            0,
        );

        // Assert
        assert!((first - 1.0).abs() < f32::EPSILON);
        assert_eq!(second, 0.0);
    }

    #[test]
    fn pool_cell_wraps_around_the_torus_when_the_origin_moves() {
        // Arrange: 場の左上隅だけを 1.0 にする
        let mut field = Field::new(96, 96);
        field.map(|x, y, _value| if x < 3 && y < 3 { 1.0 } else { 0.0 });
        let view = field.view();

        // Act: 原点を 3 セルずらすと、隅の塊は表示の右端・下端へ回り込む
        let wrapped = pool_cell(
            |x, y| view.get(x, y),
            (view.width(), view.height()),
            (3, 3),
            32,
            32,
            31,
            31,
        );

        // Assert
        assert!((wrapped - 1.0).abs() < f32::EPSILON, "got {wrapped}");
    }

    #[test]
    fn draw_leaves_the_canvas_untouched_for_an_empty_field() {
        // Arrange
        let field = Field::new(96, 96);
        let grid = DotGrid::new(32, 32);
        let mut canvas = vec![0u8; 384 * 384 * BYTES_PER_PIXEL];

        // Act
        draw_body_only(&grid, field.view(), (0.0, 0.0), &mut canvas, 384, 384);

        // Assert
        assert!(canvas.iter().all(|&byte| byte == 0));
    }

    #[test]
    fn draw_keeps_dots_inside_their_own_cell() {
        // Arrange: 1セルだけ最大値にする
        let mut field = Field::new(96, 96);
        field.map(|x, y, _value| if x < 3 && y < 3 { 1.0 } else { 0.0 });
        let grid = DotGrid::new(32, 32);
        let surface_size = 384;
        let mut canvas = vec![0u8; surface_size * surface_size * BYTES_PER_PIXEL];

        // Act
        draw_body_only(
            &grid,
            field.view(),
            (0.0, 0.0),
            &mut canvas,
            surface_size as u32,
            surface_size as u32,
        );

        // Assert: セル境界(x=12)より右には染み出さない
        let cell_size = surface_size / 32;
        let row_offset = (cell_size / 2) * surface_size;
        let outside = (row_offset + cell_size) * BYTES_PER_PIXEL;
        assert_eq!(
            canvas[outside + 3],
            0,
            "dot must not bleed into the next cell"
        );
        let inside = (row_offset + cell_size / 2) * BYTES_PER_PIXEL;
        assert_ne!(canvas[inside + 3], 0, "dot must cover its own cell center");
    }

    /// 不透明ピクセルの重心を求める。描画位置の検証に使う。
    fn drawn_centroid(canvas: &[u8], width: usize) -> (f32, f32) {
        let (mut x_total, mut y_total, mut weight) = (0.0, 0.0, 0.0);
        for (index, pixel) in canvas.chunks_exact(BYTES_PER_PIXEL).enumerate() {
            let alpha = pixel[3] as f32;
            if alpha == 0.0 {
                continue;
            }
            x_total += (index % width) as f32 * alpha;
            y_total += (index / width) as f32 * alpha;
            weight += alpha;
        }
        (x_total / weight, y_total / weight)
    }

    #[test]
    fn a_fractional_origin_shifts_the_dots_by_a_sub_cell_amount() {
        // Arrange: 1セルだけ点灯させた場を、原点を 0.0 と 0.5 で描き比べる
        let mut field = Field::new(32, 32);
        field.map(|x, y, _value| if x == 16 && y == 16 { 1.0 } else { 0.0 });
        let grid = DotGrid::new(32, 32);
        let surface = 384usize;
        let cell_size = (surface / 32) as f32;

        // Act
        let mut aligned = vec![0u8; surface * surface * BYTES_PER_PIXEL];
        draw_body_only(
            &grid,
            field.view(),
            (0.0, 0.0),
            &mut aligned,
            surface as u32,
            surface as u32,
        );
        let mut shifted = vec![0u8; surface * surface * BYTES_PER_PIXEL];
        draw_body_only(
            &grid,
            field.view(),
            (0.5, 0.0),
            &mut shifted,
            surface as u32,
            surface as u32,
        );

        // Assert: 原点を半セル進めるとドットは半セルぶん左へ動く。
        // 整数に丸める実装では両者が一致してしまい、これが揺れの原因になっていた
        let (aligned_x, aligned_y) = drawn_centroid(&aligned, surface);
        let (shifted_x, shifted_y) = drawn_centroid(&shifted, surface);
        let moved = aligned_x - shifted_x;
        assert!(
            (moved - cell_size / 2.0).abs() < 0.5,
            "expected a half-cell shift ({}), got {moved}",
            cell_size / 2.0
        );
        assert!((aligned_y - shifted_y).abs() < 0.01, "y must not move");
    }

    #[test]
    fn dots_never_spill_outside_the_grid_bounds() {
        // Arrange: 場を全点灯させ、原点をずらして端の余分な1列を描かせる
        let mut field = Field::new(32, 32);
        field.map(|_x, _y, _value| 1.0);
        let grid = DotGrid::new(32, 32);
        // 横長のサーフェスにすると、グリッドの左右に余白ができる
        let (surface_width, surface_height) = (512usize, 384usize);
        let mut canvas = vec![0u8; surface_width * surface_height * BYTES_PER_PIXEL];

        // Act
        draw_body_only(
            &grid,
            field.view(),
            (0.5, 0.5),
            &mut canvas,
            surface_width as u32,
            surface_height as u32,
        );

        // Assert: グリッドは中央 384px。その外側は透過のままでなければならない
        let bounds = grid.bounds(surface_width as u32, surface_height as u32);
        for y in 0..surface_height {
            for x in 0..surface_width {
                let inside = (x as i32) >= bounds.x
                    && (x as i32) < bounds.x + bounds.width
                    && (y as i32) >= bounds.y
                    && (y as i32) < bounds.y + bounds.height;
                if inside {
                    continue;
                }
                let alpha = canvas[(y * surface_width + x) * BYTES_PER_PIXEL + 3];
                assert_eq!(alpha, 0, "dot spilled outside the grid at ({x}, {y})");
            }
        }
    }

    #[test]
    fn an_empty_cell_touched_by_echo_still_becomes_visible() {
        // Arrange: 体が存在しないセルに echo だけを置く
        let field = Field::new(32, 32);
        let mut echo = TouchEcho::new(32, 32);
        echo.touch(&vmc_pet_body::Perturbation {
            at: CellPos { x: 16, y: 16 },
            radius: 2.0,
            amount: 0.5,
        });
        let grid = DotGrid::new(32, 32);
        let surface = 384usize;
        let mut canvas = vec![0u8; surface * surface * BYTES_PER_PIXEL];

        // Act
        grid.draw(
            field.view(),
            echo.view(),
            (0.0, 0.0),
            &mut canvas,
            surface as u32,
            surface as u32,
        );

        // Assert: 体は空でも、触れた場所にドットが現れる
        // ホバーで光らせられなかった元の問題(体を殺さないと見えない)を、echo が解決する
        assert!(
            !canvas.iter().all(|&byte| byte == 0),
            "the touched empty cell must be visible"
        );
    }

    #[test]
    fn an_echo_only_dot_uses_the_echo_color_not_the_body_color() {
        // Arrange: 体が無いセルへの echo
        let field = Field::new(32, 32);
        let mut echo = TouchEcho::new(32, 32);
        echo.touch(&vmc_pet_body::Perturbation {
            at: CellPos { x: 16, y: 16 },
            radius: 1.5,
            amount: 0.8,
        });
        let grid = DotGrid::new(32, 32);
        let surface = 384usize;
        let mut canvas = vec![0u8; surface * surface * BYTES_PER_PIXEL];

        // Act
        grid.draw(
            field.view(),
            echo.view(),
            (0.0, 0.0),
            &mut canvas,
            surface as u32,
            surface as u32,
        );

        // Assert: 中心ピクセルの色が ECHO_COLOR に近く、BODY_COLOR には寄っていない
        let cell_size = surface / 32;
        let centre = (16 * cell_size + cell_size / 2) * surface + (16 * cell_size + cell_size / 2);
        let pixel = &canvas[centre * BYTES_PER_PIXEL..centre * BYTES_PER_PIXEL + 4];
        let (g, r, a) = (pixel[1] as f32, pixel[2] as f32, pixel[3] as f32);
        assert!(a > 0.0, "the centre must be drawn");
        // premultiplied なので比率で比較する。ECHO_COLOR は赤が緑よりわずかに強い暖色
        assert!(
            r / a >= g / a,
            "an echo-only dot must lean toward the echo color"
        );
    }

    #[test]
    fn a_flushed_body_dot_leans_toward_the_pigment_color() {
        // Arrange: 体だけがあるセルに、世話を待っているときの色素を十分に溜める
        let mut field = Field::new(32, 32);
        field.map(|x, y, _value| if x == 16 && y == 16 { 1.0 } else { 0.0 });
        let echo = TouchEcho::new(32, 32);
        let mut pigment = vmc_pet_body::Pigment::new(32, 32);
        for _ in 0..900 {
            pigment.step(field.view(), 1.0);
        }
        let grid = DotGrid::new(32, 32);
        let surface = 384usize;
        let mut canvas = vec![0u8; surface * surface * BYTES_PER_PIXEL];

        // Act
        grid.draw_with_pigment(
            field.view(),
            echo.view(),
            Some(pigment.view()),
            (0.0, 0.0),
            &mut canvas,
            surface as u32,
            surface as u32,
        );

        // Assert: 体のティールは緑が赤より強いが、色づいた体は珊瑚色へ寄って赤が強くなる
        let cell_size = surface / 32;
        let centre = (16 * cell_size + cell_size / 2) * surface + (16 * cell_size + cell_size / 2);
        let pixel = &canvas[centre * BYTES_PER_PIXEL..centre * BYTES_PER_PIXEL + 4];
        let (g, r, a) = (pixel[1] as f32, pixel[2] as f32, pixel[3] as f32);
        assert!(a > 0.0, "the centre must be drawn");
        assert!(
            r > g,
            "a flushed body must lean toward the pigment color; r={r} g={g}"
        );
    }

    #[test]
    fn a_body_only_cell_keeps_the_body_color() {
        // Arrange: echo が無く、体だけがあるセル
        let mut field = Field::new(32, 32);
        field.map(|x, y, _value| if x == 16 && y == 16 { 1.0 } else { 0.0 });
        let echo = TouchEcho::new(32, 32);
        let grid = DotGrid::new(32, 32);
        let surface = 384usize;
        let mut canvas = vec![0u8; surface * surface * BYTES_PER_PIXEL];

        // Act
        grid.draw(
            field.view(),
            echo.view(),
            (0.0, 0.0),
            &mut canvas,
            surface as u32,
            surface as u32,
        );

        // Assert: BODY_COLOR は緑が赤よりはっきり強い
        let cell_size = surface / 32;
        let centre = (16 * cell_size + cell_size / 2) * surface + (16 * cell_size + cell_size / 2);
        let pixel = &canvas[centre * BYTES_PER_PIXEL..centre * BYTES_PER_PIXEL + 4];
        let (g, r, a) = (pixel[1] as f32, pixel[2] as f32, pixel[3] as f32);
        assert!(a > 0.0, "the centre must be drawn");
        assert!(
            g > r,
            "a body-only dot must keep the body color, not lean toward echo"
        );
    }
}
