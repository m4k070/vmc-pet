//! M5Stack CoreS3 上で Lenia を体として動かし、画面にドットグリッドとして描く。
//!
//! docs/M5STACK.md の step 4 に対応する。ハードウェア初期化(AXP2101電源・
//! AW9523B経由のリセットなど)は `core-s3` クレートに任せ、ここでは
//! 「場の値をどう描くか」だけを扱う。PC版(src/render/dot_grid.rs)と考え方は
//! 同じ(値が大きいほど大きな円・濃い色)だが、Rgb565・embedded-graphics 向けに
//! 書き直してある。touch_echo に相当する入力の可視化はまだ無い(IF層は step 5)。
//!
//! 実機で確認したところ、毎フレーム画面全体を `clear` してから描き直す素朴な
//! 実装は、ちらつき(黒く消えてから描き直るまでの間が見える)とフレームレートの
//! 不安定さ(消して描く総量が毎回違うため)を引き起こした。そこで
//! `DotRenderer` は前フレームの各セルの半径を覚えておき、**変化したセルだけ**
//! 消して描き直す。変わっていないセルには一切触れないため、ちらつきが無く、
//! 1フレームあたりの描画量も動きに応じて自然に少なくなる。

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;

use embedded_graphics::{
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{Circle, PrimitiveStyle},
};
use esp_backtrace as _;
use esp_hal::{
    main,
    time::{Duration, Instant},
};

use core_s3::{bsp::CoreS3DisplayResources, CoreS3};
use vmc_pet_body::{load_animal, Field, FieldView, Lenia};

esp_bootloader_esp_idf::esp_app_desc!();

/// 場の解像度。PC版と同じ 32x32(docs/DESIGN.md「場の解像度と表示解像度」参照)。
const FIELD_WIDTH: usize = 32;
const FIELD_HEIGHT: usize = 32;

/// 場を進める頻度の目安(PC版の step_rate と同じ 15/s)。
const STEP_INTERVAL: Duration = Duration::from_millis(66);

/// ドット同士が接触しないよう、セル幅に対して空ける隙間の割合(dot_grid.rsと同じ)。
const DOT_GAP_RATIO: f32 = 0.18;

/// 背景色。前フレームのドットを「消す」ときもこの色で塗る。
const BACKGROUND_COLOR: Rgb565 = Rgb565::BLACK;

/// 体の色(ティール)。PC版の BODY_COLOR と同じ狙い。
const BODY_COLOR: Rgb565 = Rgb565::new(9, 43, 20);

/// 場の総量。生物が生きているかの目安になる。
fn mass(field: &Field) -> f32 {
    let view = field.view();
    let mut total = 0.0;
    for y in 0..view.height() {
        for x in 0..view.width() {
            total += view.get(x, y);
        }
    }
    total
}

/// 場を画面いっぱいのドットグリッドとして描く。変化したセルだけを消して描き直す。
///
/// セルの縦横のピッチは独立に決める(横 = 画面幅/列数、縦 = 画面高さ/行数)。
/// 320x240 のような横長画面では、ピッチを画面短辺に合わせて正方形に揃えると
/// 左右が大きく空いてしまう。ドット自体の大きさ(半径)は重ならないよう
/// 短い方のピッチを基準にしたまま、ピッチそのものは画面いっぱいに広げることで、
/// ドットの見た目を変えずに画面を使い切る。
struct DotRenderer {
    /// 前フレームで描いた各セルの半径(ピクセル)。0 は「何も描いていない」。
    /// 初回はすべて 0 から始まるため、初回フレームは全セルが「描く」対象になる。
    previous_radius: Vec<u32>,
    cell_pitch_x: f32,
    cell_pitch_y: f32,
    max_radius: f32,
    origin_x: f32,
    origin_y: f32,
}

impl DotRenderer {
    fn new(screen_width: u32, screen_height: u32) -> Self {
        let cell_pitch_x = screen_width as f32 / FIELD_WIDTH as f32;
        let cell_pitch_y = screen_height as f32 / FIELD_HEIGHT as f32;
        // ドットの大きさは、短い方のピッチを基準に決める(重なりを防ぐため)。
        let max_radius = cell_pitch_x.min(cell_pitch_y) * 0.5 * (1.0 - DOT_GAP_RATIO);
        Self {
            previous_radius: vec![0; FIELD_WIDTH * FIELD_HEIGHT],
            cell_pitch_x,
            cell_pitch_y,
            max_radius,
            origin_x: cell_pitch_x * 0.5,
            origin_y: cell_pitch_y * 0.5,
        }
    }

    fn update<D>(&mut self, display: &mut D, field: FieldView<'_>)
    where
        D: DrawTarget<Color = Rgb565>,
    {
        for y in 0..FIELD_HEIGHT {
            for x in 0..FIELD_WIDTH {
                let value = field.get(x, y);
                // 面積が値に比例するよう半径は sqrt をとる(dot_grid.rs と同じ考え方)
                let radius = (self.max_radius * libm::sqrtf(value)) as u32;

                let index = y * FIELD_WIDTH + x;
                let previous = self.previous_radius[index];
                if radius == previous {
                    continue;
                }

                let center = Point::new(
                    (self.origin_x + self.cell_pitch_x * x as f32) as i32,
                    (self.origin_y + self.cell_pitch_y * y as f32) as i32,
                );
                if previous > 0 {
                    let _ = Circle::with_center(center, previous * 2)
                        .into_styled(PrimitiveStyle::with_fill(BACKGROUND_COLOR))
                        .draw(display);
                }
                if radius > 0 {
                    let _ = Circle::with_center(center, radius * 2)
                        .into_styled(PrimitiveStyle::with_fill(BODY_COLOR))
                        .draw(display);
                }
                self.previous_radius[index] = radius;
            }
        }
    }
}

#[main]
fn main() -> ! {
    let peripherals = esp_hal::init(esp_hal::Config::default());
    let board = CoreS3::board();

    esp_println::println!("vmc-pet-cores3: {} on {}", board.name, board.chip);
    esp_println::println!(
        "vmc-pet-cores3: display {}x{}",
        board.display.width,
        board.display.height
    );

    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 73744);

    let mut parts = CoreS3::init_display(CoreS3DisplayResources {
        i2c0: peripherals.I2C0,
        i2c_sda: peripherals.GPIO12,
        i2c_scl: peripherals.GPIO11,
        spi2: peripherals.SPI2,
        lcd_sclk: peripherals.GPIO36,
        lcd_mosi: peripherals.GPIO37,
        lcd_dc: peripherals.GPIO35,
        lcd_cs: peripherals.GPIO3,
        tf_card_cs: peripherals.GPIO4,
    })
    .expect("initialize CoreS3 display");

    parts.display.clear(BACKGROUND_COLOR).expect("clear");
    let mut renderer = DotRenderer::new(board.display.width as u32, board.display.height as u32);

    esp_println::println!("vmc-pet-cores3: loading Orbium unicaudatus");
    let animal = load_animal("O2u").expect("assets/animals.json に O2u が無い");
    esp_println::println!(
        "vmc-pet-cores3: loaded {} R={} T={}",
        animal.name,
        animal.params.radius,
        animal.params.time_divisor
    );

    let mut field = Field::new(FIELD_WIDTH, FIELD_HEIGHT);
    field.place_centered(&animal.pattern);
    let mut lenia = Lenia::new(animal.params);

    let mut step: u32 = 0;
    let mut last_step = Instant::now();
    loop {
        if last_step.elapsed() >= STEP_INTERVAL {
            lenia.step(&mut field, 1.0);
            last_step += STEP_INTERVAL;
            step += 1;

            renderer.update(&mut parts.display, field.view());

            if step % 15 == 0 {
                esp_println::println!("vmc-pet-cores3: step={step:5} mass={:.2}", mass(&field));
            }
        }
    }
}
