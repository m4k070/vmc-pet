//! 【実験】多チャンネル Lenia の生物を体として動かすファームウェアのモード。
//!
//! `VMC_PET_MULTICHANNEL=231-04` のようにビルド時の環境変数で生物を選んで書き込むと、いつものペットの
//! 代わりにこのモードで起動する。PC 版の `--multichannel` と同じく、つなぐのは元気(放置で弱る・
//! タッチで回復する)とタッチだけ(`vmc_pet_body::multichannel::MultiBody`)。M5Stack には CPU 負荷の
//! 環境シグナルが無く、気分の学習もつないでいないので、テンポは 1.0 のまま。
//!
//! 慣れ・色素・生活リズムの学習・自律コントローラ・記憶はつないでいない。記憶は読みも書きもしない
//! ので、いつものペットが覚えた生活リズムとエネルギーを上書きしない。
//!
//! 場はペットの試験一式・放置の試験と同じ 64×64・R 13(docs/experiments/rule-candidates.md)。
//! 画面(320×240)には、重心を中央に置いた 64×48 セルの窓を 5px 間隔で描く。

use alloc::vec;
use alloc::vec::Vec;

use embedded_graphics::{
    pixelcolor::{Rgb565, RgbColor},
    prelude::*,
    primitives::{Circle, PrimitiveStyle},
};
use vmc_pet_body::appearance::{DOT_GAP_RATIO, MIN_VISIBLE_VALUE};
use vmc_pet_body::multichannel::{MultiBody, load_multichannel};
use vmc_pet_body::{Camera, CellPos, Field};

/// 場の一辺と、縮めた後の R(試験一式と同じ)。
pub const FIELD_SIZE: usize = 64;
pub const RADIUS: usize = 13;

/// 画面に描く窓のセル数。320×240 に 5px 間隔で収まる。
const WINDOW_COLUMNS: usize = 64;
const WINDOW_ROWS: usize = 48;

const BACKGROUND_COLOR: Rgb565 = Rgb565::BLACK;

/// 同梱した生物を読んで体を作る。読めない・見つからない・場に収まらないときは理由を返す。
pub fn load_body(id: &str) -> Result<MultiBody, &'static str> {
    let animal = load_multichannel(id)
        .map_err(|_| "failed to parse the bundled multichannel animals")?
        .ok_or("unknown multichannel animal")?;
    MultiBody::new(animal, FIELD_SIZE, RADIUS).ok_or("the animal does not fit the field")
}

/// チャンネルの値の組を RGB565 に写す。PC 版の `DotGrid::draw_channels` と同じ決まりで、
/// チャンネル 0/1/2 を赤/緑/青の割合に、いちばん濃いチャンネルの値を明るさにする(黒い背景に描くので、
/// PC 版の不透明度を明るさとして掛ける)。
fn channel_color(red: f32, green: f32, blue: f32, visibility: f32) -> Rgb565 {
    let brightness = visibility.min(1.0) / visibility;
    let level =
        |value: f32, levels: f32| ((value * brightness).clamp(0.0, 1.0) * levels + 0.5) as u8;
    Rgb565::new(level(red, 31.0), level(green, 63.0), level(blue, 31.0))
}

/// 窓の中のセルを、変化したものだけ消して描き直す(`main.rs` の `DotRenderer` と同じ考え方)。
pub struct MultiRenderer {
    previous_radius: Vec<u32>,
    previous_color: Vec<Rgb565>,
    previous_center: Vec<Point>,
    cell_pitch_x: f32,
    cell_pitch_y: f32,
    max_radius: f32,
}

impl MultiRenderer {
    pub fn new(screen_width: u32, screen_height: u32) -> Self {
        let cells = WINDOW_COLUMNS * WINDOW_ROWS;
        let cell_pitch_x = screen_width as f32 / WINDOW_COLUMNS as f32;
        let cell_pitch_y = screen_height as f32 / WINDOW_ROWS as f32;
        let max_radius = cell_pitch_x.min(cell_pitch_y) * 0.5 * (1.0 - DOT_GAP_RATIO);
        Self {
            previous_radius: vec![0; cells],
            previous_color: vec![BACKGROUND_COLOR; cells],
            previous_center: vec![Point::zero(); cells],
            cell_pitch_x,
            cell_pitch_y,
            max_radius,
        }
    }

    /// 画面上の座標に対応する場のセル。窓の外なら `None`。
    pub fn cell_at(&self, screen_x: i32, screen_y: i32, origin: (f32, f32)) -> Option<CellPos> {
        let cell_origin_x = libm::floorf(origin.0) as i32;
        let cell_origin_y = libm::floorf(origin.1) as i32;
        let shift_x = origin.0 - cell_origin_x as f32;
        let shift_y = origin.1 - cell_origin_y as f32;
        let column = libm::floorf(screen_x as f32 / self.cell_pitch_x + shift_x) as i32;
        let row = libm::floorf(screen_y as f32 / self.cell_pitch_y + shift_y) as i32;
        let outside_window =
            column < 0 || column >= WINDOW_COLUMNS as i32 || row < 0 || row >= WINDOW_ROWS as i32;
        if outside_window {
            return None;
        }
        Some(CellPos {
            x: (column + cell_origin_x).rem_euclid(FIELD_SIZE as i32) as usize,
            y: (row + cell_origin_y).rem_euclid(FIELD_SIZE as i32) as usize,
        })
    }

    pub fn update<D>(&mut self, display: &mut D, channels: &[Vec<f32>], origin: (f32, f32))
    where
        D: DrawTarget<Color = Rgb565>,
    {
        let cell_origin_x = libm::floorf(origin.0) as i32;
        let cell_origin_y = libm::floorf(origin.1) as i32;
        let shift_x = origin.0 - cell_origin_x as f32;
        let shift_y = origin.1 - cell_origin_y as f32;
        let channel_value = |channel: usize, index: usize| {
            channels.get(channel).map_or(0.0, |values| values[index])
        };

        for row in 0..WINDOW_ROWS {
            for column in 0..WINDOW_COLUMNS {
                let field_x =
                    (column as i32 + cell_origin_x).rem_euclid(FIELD_SIZE as i32) as usize;
                let field_y = (row as i32 + cell_origin_y).rem_euclid(FIELD_SIZE as i32) as usize;
                let field_index = field_y * FIELD_SIZE + field_x;
                let red = channel_value(0, field_index);
                let green = channel_value(1, field_index);
                let blue = channel_value(2, field_index);
                let visibility = red.max(green).max(blue);

                let index = row * WINDOW_COLUMNS + column;
                let visible = visibility > MIN_VISIBLE_VALUE;
                if !visible && self.previous_radius[index] == 0 {
                    continue;
                }
                let (radius, color) = if visible {
                    (
                        (self.max_radius * libm::sqrtf(visibility)) as u32,
                        channel_color(red, green, blue, visibility),
                    )
                } else {
                    (0, BACKGROUND_COLOR)
                };
                let center = Point::new(
                    (self.cell_pitch_x * (column as f32 + 0.5 - shift_x)) as i32,
                    (self.cell_pitch_y * (row as f32 + 0.5 - shift_y)) as i32,
                );

                let previous_radius = self.previous_radius[index];
                let previous_center = self.previous_center[index];
                let unchanged = radius == previous_radius
                    && center == previous_center
                    && (radius == 0 || color == self.previous_color[index]);
                if unchanged {
                    continue;
                }
                // 消す円は「前回実際に描いた場所」に描く(`DotRenderer::update` 参照)
                if previous_radius > 0 {
                    let _ = Circle::with_center(previous_center, previous_radius * 2)
                        .into_styled(PrimitiveStyle::with_fill(BACKGROUND_COLOR))
                        .draw(display);
                }
                if radius > 0 {
                    let _ = Circle::with_center(center, radius * 2)
                        .into_styled(PrimitiveStyle::with_fill(color))
                        .draw(display);
                }
                self.previous_radius[index] = radius;
                self.previous_color[index] = color;
                self.previous_center[index] = center;
            }
        }
    }
}

/// 全チャンネルの和を1枚の場にしたもの。表示原点を重心へ寄せる(`Camera`)ためだけに使う。
pub fn follow_body(camera: &mut Camera, summed: &mut Field, body: &MultiBody) {
    let world = body.world();
    summed.map(|x, y, _| world.total(y * FIELD_SIZE + x));
    camera.follow(summed.view(), WINDOW_COLUMNS, WINDOW_ROWS);
}
