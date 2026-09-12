//! M5Stack CoreS3 上で Lenia を体として動かす、最初の検証用ファームウェア。
//!
//! docs/M5STACK.md の step 3 に対応する: 画面には何も描かず、`vmc-pet-body`
//! (no_std)を使って場を実際にステップさせ、総量をシリアルへ出す。
//! 「デスクトップ版と同じロジックが、実機の上で本当に生物を生かし続けられるか」
//! だけを確かめるための、意図的に最小限のファームウェア。

#![no_std]
#![no_main]

extern crate alloc;

use esp_hal::{
    clock::CpuClock,
    main,
    time::{Duration, Instant},
};

use esp_backtrace as _;

use vmc_pet_body::{load_animal, Field, Lenia};

esp_bootloader_esp_idf::esp_app_desc!();

/// 場の解像度。PC版と同じ 32x32(docs/DESIGN.md「場の解像度と表示解像度」参照)。
const FIELD_WIDTH: usize = 32;
const FIELD_HEIGHT: usize = 32;

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

#[main]
fn main() -> ! {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let _peripherals = esp_hal::init(config);

    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 73744);

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

    esp_println::println!("vmc-pet-cores3: initial mass={:.2}", mass(&field));

    let mut step: u32 = 0;
    loop {
        // growth_scale=1.0(気分状態は後段の課題。ここでは体そのものの生存だけを見る)
        lenia.step(&mut field, 1.0);
        step += 1;

        if step % 15 == 0 {
            esp_println::println!(
                "vmc-pet-cores3: step={step:5} mass={:.2}",
                mass(&field)
            );
        }

        let delay_start = Instant::now();
        while delay_start.elapsed() < Duration::from_millis(66) {}
    }
}
