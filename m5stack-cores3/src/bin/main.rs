//! M5Stack CoreS3 上で Lenia を体として動かし、画面にドットグリッドとして描く。
//!
//! docs/M5STACK.md の step 4・5 に対応する。ハードウェア初期化(AXP2101電源・
//! AW9523B経由のリセットなど)は `core-s3` クレートに任せ、ここでは
//! 「場の値をどう描くか」と「タッチをどう体・echoに翻訳するか」だけを扱う。
//! PC版(src/render/dot_grid.rs, src/app.rs)と考え方は同じだが、Rgb565・
//! embedded-graphics・FT6336U静電容量タッチ向けに書き直してある。
//!
//! Touch の意味づけ(体には触れないホバー、安全域の内側に採ったクリックの強さ)
//! と TouchEcho(入力の可視化。体には一切影響しない)は `vmc_pet_body` 側にある。
//! PC版と同じ実装・同じ安全性検証済みの定数を共有する
//! (docs/M5STACK.md「タッチ操作の実装」参照)。
//!
//! 実機で確認したところ、毎フレーム画面全体を `clear` してから描き直す素朴な
//! 実装は、ちらつきとフレームレートの不安定さを引き起こした。そこで
//! `DotRenderer` は前フレームの各セルの半径と色を覚えておき、**変化したセル
//! だけ**消して描き直す。

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;

use embedded_graphics::{
    pixelcolor::{Rgb565, RgbColor},
    prelude::*,
    primitives::{Circle, PrimitiveStyle},
};
use esp_backtrace as _;
use esp_hal::{
    delay::Delay,
    main,
    time::{Duration, Instant},
};

use core_s3::{
    bsp::CoreS3DisplayResources,
    touch::{Ft6336u, TouchPhase},
    CoreS3,
};
use vmc_pet_body::{
    body_perturbation_for, echo_perturbation_for, load_animal, BodyPort, CellPos, FieldView,
    LeniaBody, Touch, TouchEcho, TouchEchoView,
};

esp_bootloader_esp_idf::esp_app_desc!();

/// 場の解像度。縦は PC版と同じ 32(docs/DESIGN.md「場の解像度と表示解像度」参照)。
/// 横は画面(320x240)のアスペクト比に合わせて広げてある。43 は 320/(240/32) の
/// 近似値で、セルピッチが縦横ほぼ等しくなる(横7.44px・縦7.5px)ように選んだ。
const FIELD_WIDTH: usize = 43;
const FIELD_HEIGHT: usize = 32;

/// 場を進める頻度の目安(PC版の step_rate と同じ 15/s)。
const STEP_INTERVAL: Duration = Duration::from_millis(66);

/// タッチを読み取る間隔。touch_demo example に倣った値。
const TOUCH_POLL_INTERVAL: Duration = Duration::from_millis(40);

/// echo(入力の可視化)の、タッチ読み取りごとの減衰率。
/// PC版(app.rs の ECHO_DECAY_PER_FRAME)と同じ考え方で、体の時間とは独立に
/// 減衰させる。
const ECHO_DECAY_PER_POLL: f32 = 0.90;

/// ドット同士が接触しないよう、セル幅に対して空ける隙間の割合(dot_grid.rsと同じ)。
const DOT_GAP_RATIO: f32 = 0.18;

/// これ以下の値のセルは描画しない(dot_grid.rs の MIN_VISIBLE_VALUE と同じ)。
const MIN_VISIBLE_VALUE: f32 = 0.004;

/// 背景色。前フレームのドットを「消す」ときもこの色で塗る。
const BACKGROUND_COLOR: Rgb565 = Rgb565::BLACK;

/// 体の色(ティール)。PC版の BODY_COLOR と同じ狙い。
const BODY_COLOR: Rgb565 = Rgb565::new(9, 43, 20);

/// 入力 echo の色(暖色の白)。PC版の ECHO_COLOR と同じ狙いで、体の色とはっきり
/// 区別がつくようにしてある。
const ECHO_COLOR: Rgb565 = Rgb565::new(31, 58, 22);

/// 触れ方を、体への摂動と echo への摂動にそれぞれ翻訳して渡す。
/// PC版 app.rs の touch() と同じ役割(体に働きかける経路はここだけ)。
fn apply_touch(body: &mut LeniaBody, echo: &mut TouchEcho, touch: Touch) {
    if let Some(perturbation) = body_perturbation_for(touch) {
        body.inject(perturbation);
    }
    if let Some(perturbation) = echo_perturbation_for(touch) {
        echo.touch(&perturbation);
    }
}

fn lerp_channel(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t) as u8
}

/// 2色を `t`(0.0〜1.0)で線形補間する(dot_grid.rs の lerp_color と同じ)。
fn lerp_color(a: Rgb565, b: Rgb565, t: f32) -> Rgb565 {
    Rgb565::new(
        lerp_channel(a.r(), b.r(), t),
        lerp_channel(a.g(), b.g(), t),
        lerp_channel(a.b(), b.b(), t),
    )
}

/// 場と echo を画面いっぱいのドットグリッドとして描く。変化したセルだけを
/// 消して描き直す。座標変換(cell_at)も同じ場所に持たせ、描画と当たり判定が
/// ずれないようにしてある(PC版の DotGrid::draw / cell_at と同じ考え方)。
struct DotRenderer {
    /// 前フレームで描いた各セルの半径(ピクセル)。0 は「何も描いていない」。
    previous_radius: Vec<u32>,
    /// 前フレームで描いた各セルの色。半径が同じでも色が変わったら描き直す。
    previous_color: Vec<Rgb565>,
    cell_pitch_x: f32,
    cell_pitch_y: f32,
    max_radius: f32,
}

impl DotRenderer {
    fn new(screen_width: u32, screen_height: u32) -> Self {
        let cell_pitch_x = screen_width as f32 / FIELD_WIDTH as f32;
        let cell_pitch_y = screen_height as f32 / FIELD_HEIGHT as f32;
        // ドットの大きさは、短い方のピッチを基準に決める(重なりを防ぐため)。
        let max_radius = cell_pitch_x.min(cell_pitch_y) * 0.5 * (1.0 - DOT_GAP_RATIO);
        Self {
            previous_radius: vec![0; FIELD_WIDTH * FIELD_HEIGHT],
            previous_color: vec![BACKGROUND_COLOR; FIELD_WIDTH * FIELD_HEIGHT],
            cell_pitch_x,
            cell_pitch_y,
            max_radius,
        }
    }

    /// 画面上の座標に対応する場のセルを返す。範囲外なら `None`。
    fn cell_at(&self, screen_x: i32, screen_y: i32) -> Option<CellPos> {
        let column = (screen_x as f32 / self.cell_pitch_x) as i32;
        let row = (screen_y as f32 / self.cell_pitch_y) as i32;
        if column < 0 || column >= FIELD_WIDTH as i32 || row < 0 || row >= FIELD_HEIGHT as i32 {
            return None;
        }
        Some(CellPos {
            x: column as usize,
            y: row as usize,
        })
    }

    fn update<D>(&mut self, display: &mut D, field: FieldView<'_>, echo: TouchEchoView<'_>)
    where
        D: DrawTarget<Color = Rgb565>,
    {
        for y in 0..FIELD_HEIGHT {
            for x in 0..FIELD_WIDTH {
                let body_value = field.get(x, y);
                let echo_value = echo.get(x, y);
                // 体と echo の値のうち大きい方でドットの大きさを決める
                // (dot_grid.rs と同じ考え方)。
                let visibility = body_value.max(echo_value);
                if visibility <= MIN_VISIBLE_VALUE {
                    if self.previous_radius[y * FIELD_WIDTH + x] == 0 {
                        continue;
                    }
                }
                let radius = (self.max_radius * libm::sqrtf(visibility)) as u32;
                // echo が占める割合。体だけなら 0、echo だけなら 1 になる。
                let echo_mix = if visibility > 0.0 {
                    (echo_value / visibility).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let color = lerp_color(BODY_COLOR, ECHO_COLOR, echo_mix);

                let index = y * FIELD_WIDTH + x;
                let previous_radius = self.previous_radius[index];
                let previous_color = self.previous_color[index];
                if radius == previous_radius && (radius == 0 || color == previous_color) {
                    continue;
                }

                let center = Point::new(
                    (self.cell_pitch_x * (x as f32 + 0.5)) as i32,
                    (self.cell_pitch_y * (y as f32 + 0.5)) as i32,
                );
                if previous_radius > 0 {
                    let _ = Circle::with_center(center, previous_radius * 2)
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

    let mut touch = Ft6336u::new(parts.internal_i2c);
    let touch_ready = touch.init().is_ok();
    esp_println::println!(
        "vmc-pet-cores3: touch controller init {}",
        if touch_ready { "ok" } else { "FAILED" }
    );
    let delay = Delay::new();

    esp_println::println!("vmc-pet-cores3: loading Orbium unicaudatus");
    let animal = load_animal("O2u").expect("assets/animals.json に O2u が無い");
    esp_println::println!(
        "vmc-pet-cores3: loaded {} R={} T={}",
        animal.name,
        animal.params.radius,
        animal.params.time_divisor
    );

    let mut body = LeniaBody::new(animal, FIELD_WIDTH, FIELD_HEIGHT);
    let mut echo = TouchEcho::new(FIELD_WIDTH, FIELD_HEIGHT);
    // 指が触れている間の位置。撫でている扱いで、毎回 echo を光らせ続ける
    // (PC版 app.rs の hovering_at と同じ役割)。
    let mut touching_at: Option<CellPos> = None;

    let mut step: u32 = 0;
    let mut last_step = Instant::now();
    loop {
        if touch_ready {
            match touch.read_report() {
                Ok(report) => {
                    if let Some(event) = report.events.into_iter().flatten().next() {
                        let cell = renderer.cell_at(event.point.x, event.point.y);
                        let new_touching_at = if matches!(event.phase, TouchPhase::Up) {
                            None
                        } else {
                            cell
                        };

                        // 「新たに触れ始めた」ことは、ハードウェアの Down フェーズ
                        // ではなく、直前のポーリングで無反応(None)だったことで
                        // 判定する。描画(SPI書き込み)は重く、その間ポーリングが
                        // 止まる。ちょうどその間に押して離す速いタップが完結すると、
                        // Down フェーズのサンプルを一度も読めないまま Move や Up
                        // だけを見ることになり、Down 頼りの判定ではタッチを丸ごと
                        // 見逃していた。
                        if touching_at.is_none() {
                            if let Some(at) = new_touching_at {
                                apply_touch(&mut body, &mut echo, Touch::Click { at });
                            }
                        }
                        touching_at = new_touching_at;
                    } else {
                        touching_at = None;
                    }
                }
                Err(_) => touching_at = None,
            }
        }

        if let Some(at) = touching_at {
            apply_touch(&mut body, &mut echo, Touch::Hover { at });
        }
        echo.decay(ECHO_DECAY_PER_POLL);

        // 描画(SPIへの書き込み)は重く、体が実際に1ステップ進んだときだけ行う。
        // タッチのサンプリング(上のポーリング)とは頻度を分離してある。
        // 以前ここを毎ポーリング(40ms)無条件に呼んでいたため、描画頻度が
        // 実質2倍近くに増え、フレームレート全体が悪化していた。
        if last_step.elapsed() >= STEP_INTERVAL {
            body.step();
            last_step += STEP_INTERVAL;
            step += 1;

            renderer.update(&mut parts.display, body.observe(), echo.view());

            if step % 15 == 0 {
                esp_println::println!(
                    "vmc-pet-cores3: step={step:5} mass={:.2} energy={:.2}",
                    body.mass(),
                    body.energy()
                );
            }
        }

        delay.delay(TOUCH_POLL_INTERVAL);
    }
}
