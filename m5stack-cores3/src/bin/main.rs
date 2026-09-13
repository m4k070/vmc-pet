//! M5Stack CoreS3 上で Lenia を体として動かし、画面にドットグリッドとして描く。
//!
//! docs/M5STACK.md の step 4・5 に対応する。ハードウェア初期化(AXP2101電源・
//! AW9523B経由のリセットなど)は `core-s3` クレートに任せ、ここでは
//! 「場の値をどう描くか」と「タッチをどう体・echoに翻訳するか」だけを扱う。
//! PC版(src/render/dot_grid.rs, src/app.rs)と考え方は同じだが、Rgb565・
//! embedded-graphics・FT6336U静電容量タッチ向けに書き直してある。
//!
//! 体(世界モデル)・入力の翻訳・崩壊検知・echo(入力の可視化。体には一切
//! 影響しない)は `vmc_pet_body::Pet` にまとめてある。PC版(app.rs)と完全に
//! 同じ実装・同じ安全性検証済みの定数を共有する(docs/M5STACK.md「PC/M5Stack
//! どちらでも世界モデルを動かせるようコードを整理する」参照)。ここが持つのは
//! 「いつ進めるか」(esp_hal::time)と「どう入力を受け取り、どう描くか」
//! (FT6336U・embedded-graphics)だけ。
//!
//! 実機で確認したところ、毎フレーム画面全体を `clear` してから描き直す素朴な
//! 実装は、ちらつきとフレームレートの不安定さを引き起こした。そこで
//! `DotRenderer` は前フレームの各セルの半径と色を覚えておき、**変化したセル
//! だけ**消して描き直す。
//!
//! 表示の原点を重心へ寄せる `Camera` も `vmc_pet_body` 側にあり、PC版
//! (`src/app.rs`)と同じものを使う。`DotRenderer` の前フレーム比較(dirty-cell
//! tracking)は「今この画面位置に何が描かれているか」を追跡するだけなので、
//! カメラが動いて画面位置と場のセルの対応がずれても壊れない。

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;
use core::cell::RefCell;

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
    CoreS3,
    bsp::CoreS3DisplayResources,
    touch::{Ft6336u, TouchPhase},
};
use embedded_hal_bus::i2c::RefCellDevice;
use vmc_pet_body::{Camera, CellPos, FieldView, Pet, PigmentView, TouchEchoView, load_animal};
use vmc_pet_cores3::{clock::Clock, persistence::MemoryStore};

use vmc_pet_body::appearance::{Color, DOT_GAP_RATIO, appearance_of};

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

/// 記憶をフラッシュへ書き出す間隔。PC版(app.rs の SAVE_INTERVAL)と同じ30秒。
///
/// フラッシュの消去回数の上限(おおむね10万回)に対して安全かどうかは
/// 素朴には成り立たない。`persistence` が44バイトのレコードを93区画に
/// 並べて消去を93回に1回へ減らしているので、30秒ごとでも
/// 100000 × 93 × 30秒 ≒ 約8.8年 もつ。詳しい理屈は persistence.rs 参照。
const SAVE_INTERVAL: Duration = Duration::from_secs(30);

/// RTC から時刻を読んで `Pet::tick_clock` に渡す間隔。
const CLOCK_READ_INTERVAL: Duration = Duration::from_secs(1);

/// 気分を固定したプレビュー用ファームウェアにするときの指定(ビルド時の環境変数)。
///
/// `VMC_PET_PREVIEW_MOOD=waiting cargo run --release` のように書き込むと、生活リズムを
/// 覚えるのを待たずに、その気分(lively / waiting / disappointed)の見た目を確かめられる。
/// プレビュー中は記憶(フラッシュ)を読みも書きもしない。本物のペットが覚えた生活
/// リズムとエネルギーを上書きしないため。見終わったら指定なしで書き込み直す。
const PREVIEW_MOOD: Option<&str> = option_env!("VMC_PET_PREVIEW_MOOD");

/// echo(入力の可視化)の、タッチ読み取りごとの減衰率。
/// PC版(app.rs の ECHO_DECAY_PER_FRAME)と同じ考え方で、体の時間とは独立に
/// 減衰させる。
const ECHO_DECAY_PER_POLL: f32 = 0.90;

/// 背景色。前フレームのドットを「消す」ときもこの色で塗る。
const BACKGROUND_COLOR: Rgb565 = Rgb565::BLACK;

/// 共有の色(0.0..=1.0)を RGB565 に写す。
///
/// 以前は色ごとに RGB565 の値を手で書いていて、体の色が PC版より約2割暗く、
/// 色素の色も少しずれていた(`vmc_pet_body::appearance` 参照)。色の定義と混ぜ方は
/// PC版と共有し、ここは変換だけを持つ。
fn to_rgb565(color: Color) -> Rgb565 {
    let channel = |value: f32, levels: f32| (value.clamp(0.0, 1.0) * levels + 0.5) as u8;
    Rgb565::new(
        channel(color.red, 31.0),
        channel(color.green, 63.0),
        channel(color.blue, 31.0),
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
    /// 前フレームで実際に円を描いた画面座標。カメラが動く(重心追従で毎フレーム
    /// サブピクセル単位にずれる)ようになったため、半径・色が同じでも「前回
    /// 実際に描いた場所」を覚えておかないと、消す円を今回の(ずれた)位置に
    /// 描いてしまい、古い位置のドットを消し損ねて残像になる。
    previous_center: Vec<Point>,
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
            previous_center: vec![Point::zero(); FIELD_WIDTH * FIELD_HEIGHT],
            cell_pitch_x,
            cell_pitch_y,
            max_radius,
        }
    }

    /// 画面上の座標に対応する場のセルを返す。範囲外なら `None`。
    ///
    /// `origin`(`Camera::origin()`)の整数部がどの場のセルから読むかを、
    /// 小数部が画面上のスロットとのずれをどれだけ補正するかを決める。
    /// `DotRenderer::update` が描く位置とずれないよう、同じ変換をここでも行う
    /// (PC版の `DotGrid::cell_at` と同じ考え方)。
    fn cell_at(&self, screen_x: i32, screen_y: i32, origin: (f32, f32)) -> Option<CellPos> {
        let cell_origin_x = libm::floorf(origin.0) as i32;
        let cell_origin_y = libm::floorf(origin.1) as i32;
        let shift_x = origin.0 - cell_origin_x as f32;
        let shift_y = origin.1 - cell_origin_y as f32;

        let column = libm::floorf(screen_x as f32 / self.cell_pitch_x + shift_x) as i32;
        let row = libm::floorf(screen_y as f32 / self.cell_pitch_y + shift_y) as i32;
        if column < 0 || column >= FIELD_WIDTH as i32 || row < 0 || row >= FIELD_HEIGHT as i32 {
            return None;
        }
        let field_x = (column + cell_origin_x).rem_euclid(FIELD_WIDTH as i32);
        let field_y = (row + cell_origin_y).rem_euclid(FIELD_HEIGHT as i32);
        Some(CellPos {
            x: field_x as usize,
            y: field_y as usize,
        })
    }

    fn update<D>(
        &mut self,
        display: &mut D,
        field: FieldView<'_>,
        echo: TouchEchoView<'_>,
        pigment: PigmentView<'_>,
        origin: (f32, f32),
    ) where
        D: DrawTarget<Color = Rgb565>,
    {
        let cell_origin_x = libm::floorf(origin.0) as i32;
        let cell_origin_y = libm::floorf(origin.1) as i32;
        let shift_x = origin.0 - cell_origin_x as f32;
        let shift_y = origin.1 - cell_origin_y as f32;

        for row in 0..FIELD_HEIGHT {
            for column in 0..FIELD_WIDTH {
                // 画面のスロット(column, row)には、カメラの原点ぶんだけずらした
                // 場のセルを表示する(dot_grid.rs の pool_cell と同じ考え方だが、
                // 場と画面が1:1なのでプーリングは要らない)。
                let field_x =
                    ((column as i32 + cell_origin_x).rem_euclid(FIELD_WIDTH as i32)) as usize;
                let field_y =
                    ((row as i32 + cell_origin_y).rem_euclid(FIELD_HEIGHT as i32)) as usize;
                let body_value = field.get(field_x, field_y);
                let echo_value = echo.get(field_x, field_y);
                // 1セルの見え方(大きさ・色)は PC版と共有する決まりに従う
                // (`vmc_pet_body::appearance`)。ここが持つのは RGB565 への変換と
                // ドットの配置だけ。
                let tint = pigment.get(field_x, field_y);
                let appearance = appearance_of(body_value, echo_value, tint);
                let index = row * FIELD_WIDTH + column;
                if appearance.is_none() && self.previous_radius[index] == 0 {
                    continue;
                }
                // 見えなくなったセルは半径 0 として、前回描いたドットを消すだけにする
                let (radius, color) = match appearance {
                    Some(appearance) => (
                        (self.max_radius * appearance.size) as u32,
                        to_rgb565(appearance.color),
                    ),
                    None => (0, BACKGROUND_COLOR),
                };

                // 場のセルから画面座標への変換は、選ぶセル(cell_origin)と
                // 画面上の位置(shift)を別々にずらす。これにより、生物の実際の
                // 動きが1セル未満の単位でも滑らかに見える(dot_grid.rs 参照)。
                let center = Point::new(
                    (self.cell_pitch_x * (column as f32 + 0.5 - shift_x)) as i32,
                    (self.cell_pitch_y * (row as f32 + 0.5 - shift_y)) as i32,
                );

                let previous_radius = self.previous_radius[index];
                let previous_color = self.previous_color[index];
                let previous_center = self.previous_center[index];
                // カメラが動くと、半径・色が変わらないセルでも画面上の位置
                // (center)だけがずれる。これを比較に含めないと、消す円が
                // 「前回実際に描いた場所」ではなく「今回の(ずれた)位置」に
                // 描かれてしまい、古い位置のドットを消し損ねて残像になる
                // (実機でユーザーが確認して見つかったバグ)。
                if radius == previous_radius
                    && center == previous_center
                    && (radius == 0 || color == previous_color)
                {
                    continue;
                }

                // erase → draw の2回描画を1回にまとめる最適化(同心円の外接矩形を
                // 自前の距離判定で塗る案)を試したが、実機で「生き物が動いた後ろに
                // 薄く跡が残る」問題が起きた。1行ずつの書き込みに分けても再現したため
                // SPI側の複数行アドレスウィンドウが原因ではなく、embedded-graphics の
                // `Circle` と自前のラスタライズが1ピクセル単位で厳密には一致していない
                // ことが原因と見ている。描画コストはそもそも1ステップの1割未満
                // (docs/M5STACK.md 参照)で最適化の価値が薄いため、正しさを優先し
                // 素直な2回描画に戻した。消す円は「前回実際に描いた場所」
                // (previous_center)に描く。今回の center に描くと、カメラが
                // 動いた分だけ消し損ねる。
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

#[main]
fn main() -> ! {
    // CPU クロックはビルド時の環境変数 `VMC_PET_CPU_CLOCK`(80 / 160 / 240)で選べる。
    // 既定は esp-hal の既定値の 80MHz。240MHz で書き込んだ後に USB から消えたことが
    // あり、原因を調べるための切り替え(docs/M5STACK.md「240MHz で USB から消えた件の調査」)。
    let cpu_clock = match option_env!("VMC_PET_CPU_CLOCK") {
        Some("240") => esp_hal::clock::CpuClock::_240MHz,
        Some("160") => esp_hal::clock::CpuClock::_160MHz,
        _ => esp_hal::clock::CpuClock::_80MHz,
    };
    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(cpu_clock));
    let board = CoreS3::board();

    esp_println::println!("vmc-pet-cores3: {} on {}", board.name, board.chip);
    // どう起動したか(USB 経由のソフトウェアリセットか、物理リセットか、電源投入か)と、
    // 実際に動いている CPU クロックを残す。起動の仕方で振る舞いが変わる問題の切り分けに使う。
    esp_println::println!(
        "vmc-pet-cores3: cpu clock {} MHz, reset reason {:?}",
        esp_hal::clock::cpu_clock().as_mhz(),
        esp_hal::system::reset_reason()
    );
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

    // タッチコントローラ(FT6336U)と RTC(BM8563)は同じ内部I²Cバスに
    // いるので、バスを共有する。`main` は決して return しないため、
    // ここで作った `RefCell` は実質 'static として借り出せる。
    let internal_i2c = RefCell::new(parts.internal_i2c);

    let mut touch = Ft6336u::new(RefCellDevice::new(&internal_i2c));
    let touch_ready = touch.init().is_ok();
    esp_println::println!(
        "vmc-pet-cores3: touch controller init {}",
        if touch_ready { "ok" } else { "FAILED" }
    );
    let delay = Delay::new();

    // 電源が切れている間も進む時計。これが無いと「停止していた時間」が
    // 分からず、記憶を持ち越しても再起動のたびに時間が止まったままになる。
    let mut clock = match Clock::new(RefCellDevice::new(&internal_i2c)) {
        Ok(clock) => {
            if clock.lost_its_place() {
                esp_println::println!(
                    "vmc-pet-cores3: the RTC lost its time; starting the clock over \
                     (no offline decay will be applied this boot)"
                );
            }
            Some(clock)
        }
        Err(_) => {
            esp_println::println!("vmc-pet-cores3: RTC unavailable; time away will be ignored");
            None
        }
    };

    // 記憶の置き場所(フラッシュの `pet` パーティション)。
    let preview = PREVIEW_MOOD.and_then(vmc_pet_body::MoodState::from_name);
    if let (Some(name), None) = (PREVIEW_MOOD, preview) {
        esp_println::println!(
            "vmc-pet-cores3: unknown VMC_PET_PREVIEW_MOOD={name} \
             (expected lively, waiting or disappointed); running normally"
        );
    }
    // プレビュー中は記憶の置き場所を作らない。復元も保存も、置き場所があるときだけ
    // 行う作りなので、これだけで本物のペットの記憶に一切触れなくなる。
    let mut memory_store = if let Some(state) = preview {
        esp_println::println!(
            "vmc-pet-cores3: previewing the {} mood; memory is neither loaded nor saved",
            state.name()
        );
        None
    } else {
        match MemoryStore::new(peripherals.FLASH) {
            Ok(store) => {
                esp_println::println!(
                    "vmc-pet-cores3: memory store ready; {}/{} slots used",
                    store.used_slots(),
                    store.slots()
                );
                Some(store)
            }
            Err(error) => {
                esp_println::println!(
                    "vmc-pet-cores3: memory unavailable ({error:?}); starting fresh"
                );
                None
            }
        }
    };

    esp_println::println!("vmc-pet-cores3: loading Orbium unicaudatus");
    let animal = load_animal("O2u").expect("assets/animals.json に O2u が無い");
    esp_println::println!(
        "vmc-pet-cores3: loaded {} R={} T={}",
        animal.name,
        animal.params.radius,
        animal.params.time_divisor
    );

    let mut pet = Pet::new(animal, FIELD_WIDTH, FIELD_HEIGHT);
    if let Some(state) = preview {
        let (anticipation, disappointment) = state.anticipation_and_disappointment();
        pet.pin_mood(anticipation, disappointment);
    }

    // 前回の続きから始める。時計が無い・記憶が無い・時計が巻き戻っている
    // のいずれでも、単に「新品として始まる」だけで先へ進む。
    if let (Some(store), Some(clock)) = (memory_store.as_ref(), clock.as_mut())
        && let (Some(saved), Ok(now)) = (store.load(), clock.now_unix_seconds())
    {
        let before = saved.memory.energy;
        let seconds_away = saved.seconds_away(now);
        pet.restore(saved.memory, seconds_away);
        // 秒も出すのは、RTC がちゃんと進んでいるかを実機で確かめる手立てが
        // これしかないため。時間単位だけだと、短い再起動が全部 0.0h に
        // 見えてしまい、時計が止まっているのと区別がつかない。
        esp_println::println!(
            "vmc-pet-cores3: resumed after {:.1}h ({seconds_away:.0}s) away; \
             energy {before:.2} -> {:.2}",
            seconds_away / 3600.0,
            pet.energy()
        );
    }

    // 生物が場の端で分断されて見えないよう、表示原点を重心へ寄せる
    // (PC版 app.rs の Camera と同じもの。docs/DESIGN.md参照)。
    let mut camera = Camera::new();

    let mut step: u32 = 0;
    let mut last_step = Instant::now();
    let mut last_saved = Instant::now();
    let mut last_clock_read = Instant::now();
    loop {
        if touch_ready {
            match touch.read_report() {
                Ok(report) => {
                    if let Some(event) = report.events.into_iter().flatten().next() {
                        let cell = renderer.cell_at(event.point.x, event.point.y, camera.origin());
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
                        match new_touching_at {
                            Some(at) if pet.touching_at().is_none() => pet.click(at),
                            Some(at) => pet.hover_at(Some(at)),
                            None => pet.leave(),
                        }
                    } else {
                        pet.leave();
                    }
                }
                Err(_) => pet.leave(),
            }
        }

        pet.tick_input(ECHO_DECAY_PER_POLL);

        // 描画(SPIへの書き込み)は重く、体が実際に1ステップ進んだときだけ行う。
        // タッチのサンプリング(上のポーリング)とは頻度を分離してある。
        // 以前ここを毎ポーリング(40ms)無条件に呼んでいたため、描画頻度が
        // 実質2倍近くに増え、フレームレート全体が悪化していた。
        if last_step.elapsed() >= STEP_INTERVAL {
            let collapsed = pet.step();
            last_step += STEP_INTERVAL;
            step += 1;

            camera.follow(pet.observe(), FIELD_WIDTH, FIELD_HEIGHT);
            renderer.update(
                &mut parts.display,
                pet.observe(),
                pet.echo_view(),
                pet.pigment_view(),
                camera.origin(),
            );

            if collapsed {
                esp_println::println!("vmc-pet-cores3: the body collapsed; reviving");
            }
            if step.is_multiple_of(15) {
                esp_println::println!(
                    "vmc-pet-cores3: step={step:5} mass={:.2} energy={:.2} anticipation={:.2} disappointment={:.2}",
                    pet.mass(),
                    pet.energy(),
                    pet.anticipation(),
                    pet.disappointment()
                );
            }
        }

        // 保存は体を進めるのとは別の頻度で行う。フラッシュへの書き込みは
        // CPU を一瞬止める(同じフラッシュから命令を読んでいるため)ので、
        // 描画と同じループの中で、体のステップとは独立に間隔を測る。
        if last_saved.elapsed() >= SAVE_INTERVAL {
            last_saved = Instant::now();
            if let (Some(store), Some(clock)) = (memory_store.as_mut(), clock.as_mut())
                && let Ok(now) = clock.now_unix_seconds()
            {
                store.save(pet.memory(), now);
            }
        }

        // 世話がいつ来るかを学ぶため、時刻を知らせる。RTC は内部I²Cバスをタッチと
        // 共有しているので、読むのは1秒に1回に抑える(学習の単位は1分なので足りる)。
        if last_clock_read.elapsed() >= CLOCK_READ_INTERVAL {
            last_clock_read = Instant::now();
            if let Some(Ok(now)) = clock.as_mut().map(|clock| clock.now_unix_seconds()) {
                pet.tick_clock(now);
            }
        }

        delay.delay(TOUCH_POLL_INTERVAL);
    }
}
