//! M5Stack CoreS3 上で Lenia を体として動かし、画面にドットグリッドとして描く。
//!
//! docs/M5STACK.md の step 4 に対応する。ハードウェア初期化(AXP2101電源・
//! AW9523B経由のリセットなど)は `core-s3` クレートに任せ、ここでは
//! 「場の値をどう描くか」だけを扱う。PC版(src/render/dot_grid.rs)と考え方は
//! 同じ(値が大きいほど大きな円・濃い色)だが、Rgb565・embedded-graphics 向けに
//! 書き直してある。touch_echo に相当する入力の可視化はまだ無い(IF層は step 5)。

#![no_std]
#![no_main]

extern crate alloc;

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

/// 場をドットグリッドとして描く。中心寄せしたうえで、値が大きいほど大きな円を描く。
/// `display` は毎回 `clear` 済みであることを前提に、上書きで描く。
fn draw_field<D>(display: &mut D, field: FieldView<'_>)
where
    D: DrawTarget<Color = Rgb565>,
{
    let (screen_w, screen_h) = (display.bounding_box().size.width, display.bounding_box().size.height);
    let cell_size = (screen_w as usize / FIELD_WIDTH).min(screen_h as usize / FIELD_HEIGHT) as i32;
    if cell_size == 0 {
        return;
    }
    let grid_w = cell_size * FIELD_WIDTH as i32;
    let grid_h = cell_size * FIELD_HEIGHT as i32;
    let origin_x = (screen_w as i32 - grid_w) / 2;
    let origin_y = (screen_h as i32 - grid_h) / 2;
    let max_radius = cell_size as f32 * 0.5 * (1.0 - DOT_GAP_RATIO);

    for y in 0..FIELD_HEIGHT {
        for x in 0..FIELD_WIDTH {
            let value = field.get(x, y);
            if value <= 0.004 {
                continue;
            }
            // 面積が値に比例するよう半径は sqrt をとる(dot_grid.rs と同じ考え方)
            let radius = (max_radius * libm::sqrtf(value)) as u32;
            if radius == 0 {
                continue;
            }
            let center_x = origin_x + cell_size * x as i32 + cell_size / 2;
            let center_y = origin_y + cell_size * y as i32 + cell_size / 2;
            let _ = Circle::with_center(Point::new(center_x, center_y), radius * 2)
                .into_styled(PrimitiveStyle::with_fill(BODY_COLOR))
                .draw(display);
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

    parts.display.clear(Rgb565::BLACK).expect("clear");

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

            parts.display.clear(Rgb565::BLACK).expect("clear");
            draw_field(&mut parts.display, field.view());

            if step % 15 == 0 {
                esp_println::println!("vmc-pet-cores3: step={step:5} mass={:.2}", mass(&field));
            }
        }
    }
}
