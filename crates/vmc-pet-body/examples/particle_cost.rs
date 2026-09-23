//! 【スパイク】粒子の体を M5Stack に載せられるか、PC の比から見積もる。
//!
//! M5Stack(CoreS3, 240MHz)は、いまの Lenia の体(O2u、43×32)を 15 ステップ
//! 約 1.33 秒で進めている(docs/M5STACK.md「フレームレート改善を試みた記録」)。
//! 目標は 15 ステップ ≒ 1.0 秒なので、体が軽くなれば初めて届く。粒子の体は
//! 1 ステップがいまの Lenia の約 7 分の 1 と見積もられていた
//! (docs/experiments/body-candidates.md「閉じた場を動き回る粒子の体を探す」)が、
//! その見積もりは `ParticleWorld::step` だけを 32 セルの箱で測ったもので、
//! ペットに繋いだ後の体(`ParticleBody::step` = 誘いの判定 + 進行 + ラスタ化)では
//! 測っていない。ここでは同じ PC・同じ条件で、
//!
//! 1. `ParticleBody::step`(ペットが実際に回すもの)
//! 2. その内訳(誘いの判定 `observe` / 進行 / ラスタ化)
//! 3. 基準となる `LeniaBody::step`(M5Stack と同じ 43×32)
//!
//! を測り、M5Stack での1ステップを PC 比から見積もる。内訳を測るのは、間に合わな
//! かったときに何を削れるかを先に知っておくため(誘いの判定は preview では1秒ごと
//! だった。毎ステップ測るのを戻せば、その分は 15 分の 1 になる)。
//!
//! ```sh
//! cargo run --release -p vmc-pet-body --example particle_cost
//! ```

use std::time::Instant;

use vmc_pet_body::particles::{ParticleParams, ParticleWorld};
use vmc_pet_body::particles_body::{ParticleBody, BOX_SIZE};
use vmc_pet_body::LeniaBody;

/// ペットに繋いだ粒子の体の候補番号(docs/experiments/body-candidates.md)。
const PARTICLE_SEED: u64 = 1091;
/// 計測するステップ数。20000 ステップは試験と同じ長さ。
const MEASURED_STEPS: u32 = 20_000;
/// 測りはじめる前に捨てるステップ数(塊が落ち着くまで)。
const WARMUP_STEPS: u32 = 300;
/// Lenia は1ステップが重いので、同じ秒数に収まるよう少なめに回す。
const LENIA_MEASURED_STEPS: u32 = 5_000;
/// M5Stack の体の場(画面のアスペクト比に合わせた大きさ)。
const M5_FIELD_WIDTH: usize = 43;
const M5_FIELD_HEIGHT: usize = 32;
/// M5Stack(240MHz)で実測した、いまの Lenia の1ステップ(秒)。
/// 15 ステップ 1.33 秒(docs/M5STACK.md)から。
const M5_LENIA_STEP_SECONDS: f64 = 1.33 / 15.0;
/// 体のステップの目標(15 ステップ/秒)。
const TARGET_STEP_SECONDS: f64 = 1.0 / 15.0;

fn main() {
    let params = ParticleParams::from_seed(PARTICLE_SEED);
    println!(
        "粒子の体 {PARTICLE_SEED}: 粒子 {} 個(種類 {} × {} 個)、箱 {BOX_SIZE} セル\n",
        params.count(),
        params.types,
        params.per_type
    );

    let body_step = measure_particle_body();
    let world_step = measure_world_step();
    let observe = measure_observe();
    let rasterize = body_step - world_step - observe;
    let lenia_step = measure_lenia_step();

    println!("PC(1スレッド)の1ステップ");
    println!("| 測ったもの | µs | Lenia 比 |");
    println!("|---|---|---|");
    print_row("粒子の体(ParticleBody::step)", body_step, lenia_step);
    print_row("  うち誘いの判定(observe)", observe, lenia_step);
    print_row("  うち進行(step_at_tempo)", world_step, lenia_step);
    print_row("  うちラスタ化(推定: 残り)", rasterize, lenia_step);
    print_row("Lenia の体(43×32、O2u)", lenia_step, lenia_step);

    let ratio = body_step / lenia_step;
    let estimated = M5_LENIA_STEP_SECONDS * ratio;
    println!(
        "\nM5Stack の見積もり: {:.1} ms/ステップ(いまの Lenia の実測 {:.1} ms × {:.2})",
        estimated * 1e3,
        M5_LENIA_STEP_SECONDS * 1e3,
        ratio
    );
    println!(
        "15 ステップ {:.2} 秒(目標 1.00 秒、いまの体は 1.33 秒)。目標に{}",
        estimated * 15.0,
        if estimated <= TARGET_STEP_SECONDS {
            "収まる"
        } else {
            "収まらない"
        }
    );
}

fn print_row(label: &str, seconds: f64, lenia_seconds: f64) {
    println!(
        "| {label} | {:.1} | ×{:.3} |",
        seconds * 1e6,
        seconds / lenia_seconds
    );
}

/// ペットが実際に回す1ステップ(誘いの判定 + 進行 + ラスタ化)。
fn measure_particle_body() -> f64 {
    let mut body = ParticleBody::new(PARTICLE_SEED);
    for _ in 0..WARMUP_STEPS {
        body.step();
    }
    let timer = Instant::now();
    for _ in 0..MEASURED_STEPS {
        body.step();
    }
    timer.elapsed().as_secs_f64() / f64::from(MEASURED_STEPS)
}

/// 進行だけ(粒子どうしの力と壁。ペットに繋ぐ前に測っていたもの)。
fn measure_world_step() -> f64 {
    let mut world = started_world();
    let timer = Instant::now();
    for _ in 0..MEASURED_STEPS {
        world.step();
    }
    timer.elapsed().as_secs_f64() / f64::from(MEASURED_STEPS)
}

/// 誘いの判定(最大の塊とはぐれた粒子を数える)。
fn measure_observe() -> f64 {
    let mut world = started_world();
    let timer = Instant::now();
    for _ in 0..MEASURED_STEPS {
        // 塊は動き続けるので、毎回同じ場を見ないよう1ステップずつ進めながら測る。
        // その分(`measure_world_step` と同じ処理)は後で差し引く。
        world.step();
        let observed = world.observe();
        core::hint::black_box(observed.strays);
    }
    let with_step = timer.elapsed().as_secs_f64() / f64::from(MEASURED_STEPS);
    with_step - measure_world_step()
}

fn started_world() -> ParticleWorld {
    let mut world = ParticleWorld::new(ParticleParams::from_seed(PARTICLE_SEED), BOX_SIZE, 1);
    for _ in 0..WARMUP_STEPS {
        world.step();
    }
    world
}

/// 基準: M5Stack と同じ場(43×32)で動くいまの体。
fn measure_lenia_step() -> f64 {
    let animal = vmc_pet_body::load_animal("O2u").expect("O2u を読めない");
    let mut body = LeniaBody::new(animal, M5_FIELD_WIDTH, M5_FIELD_HEIGHT);
    for _ in 0..WARMUP_STEPS {
        body.step();
    }
    let timer = Instant::now();
    for _ in 0..LENIA_MEASURED_STEPS {
        body.step();
    }
    let seconds = timer.elapsed().as_secs_f64() / f64::from(LENIA_MEASURED_STEPS);
    // 場の大きさを取り違えていないことを、総量が健全な範囲にあることで確かめる。
    let mass = body.mass();
    assert!(mass > 10.0, "Lenia の体が崩壊している(総量 {mass})");
    seconds
}
