//! 【実験】多チャンネル Lenia にすると、1ステップの計算がどれだけ重くなるか。
//!
//! 多チャンネル Lenia(Chan 2020, "Lenia and Expanded Universe")は、チャンネル数 c、
//! チャンネルごとの自己カーネル数 ks、チャンネルの組ごとの相互カーネル数 kx に対して、
//! nk = ks·c + kx·c(c−1) 本のカーネルを持つ。1ステップの重さは、ほぼ「カーネルごとに、元の
//! チャンネルを疎に畳み込み、全セルで成長関数を評価する」ことで決まる。
//!
//! 多チャンネルの生物はまだ無いので、1本ぶんを「元のチャンネルの複製に `Lenia::step_at_tempo`
//! を1回かける」ことで模擬し、いまの体と同じ畳み込みのコードで時間を測る。各チャンネルの中身は
//! 実際の生物を動かした場にする(疎な畳み込みの重さは、生きているセル数で決まるため)。
//! カーネルの半径はすべて生物の R にそろえる(論文では r_k·R、r_k ≤ 1 なので、これが上限)。
//!
//! M5Stack の値は、実測(O2u、43×32、1本で `body.step()` 約62ms。docs/M5STACK.md)に、PC で
//! 測った比を掛けた見積もり。CPU の違い(キャッシュ・浮動小数点)で比が変わる可能性がある。
//!
//! 比べるため、yuca の s613 のカエル(Glaberish、R = 31、128×128)の1ステップも測れる。パターンは
//! `glaberish_trial.rs` と同じく、yuca の zoo を JSON に書き出したものを使う。
//!
//! 使い方: `cargo run --release -p vmc-pet-body --example multichannel_cost [-- <パターンの JSON が
//! あるディレクトリ>]`。並列には走らせない。

use std::time::Instant;

use std::fs;
use std::path::Path;

use serde::Deserialize;
use vmc_pet_body::lenia::{GrowthMapping, KernelCore, LeniaParams};
use vmc_pet_body::{load_animal, Animal, Field, GrowthFunction, Lenia};

/// 比べるための s613 のカエル(`glaberish_trial.rs` と同じ規則とパターン)を置く場。
const FROG_FIELD: usize = 128;
const FROG_REPETITIONS: usize = 200;

/// yuca の zoo のパターンを書き出した JSON。
#[derive(Deserialize)]
struct PatternFile {
    rows: usize,
    cols: usize,
    values: Vec<f32>,
}

/// yuca の s613 の規則(`glaberish_trial.rs` の `s613` と同じ)。
fn s613() -> (Lenia, GrowthFunction, GrowthFunction) {
    let params = LeniaParams {
        radius: 31,
        time_divisor: 10.0,
        kernel_peaks: vec![1.0],
        kernel_core: KernelCore::Polynomial,
        growth_center: 0.0621,
        growth_width: 0.0088,
        growth_mapping: GrowthMapping::Exponential,
    };
    let rings: [(f32, f32, f32); 3] = [
        (0.5, 0.093809, 0.033),
        (1.0, 0.28143, 0.033),
        (2.0 / 3.0, 0.46904, 0.033),
    ];
    let profile = move |distance: f32| {
        rings
            .iter()
            .map(|(height, center, width)| {
                height * (-((distance - center) / width).powi(2) / 2.0).exp()
            })
            .sum::<f32>()
            .clamp(0.0, 1.0)
    };
    let genesis = GrowthFunction {
        mapping: GrowthMapping::Exponential,
        center: 0.0621,
        width: 0.0088,
    };
    let persistence = GrowthFunction {
        mapping: GrowthMapping::Exponential,
        center: 0.2151,
        width: 0.0369,
    };
    (
        Lenia::with_radial_profile(params, profile),
        genesis,
        persistence,
    )
}

/// s613 のカエルを落ち着かせてから、1ステップの時間(中央値、µs)と生きているセル数を測る。
fn time_frog(directory: &Path, name: &str) -> (f32, usize) {
    let text = fs::read_to_string(directory.join(format!("{name}.json")))
        .unwrap_or_else(|_| panic!("{name}.json を読めない"));
    let pattern: PatternFile = serde_json::from_str(&text).expect("パターンの JSON が壊れている");
    let top = (FROG_FIELD - pattern.rows) / 2;
    let left = (FROG_FIELD - pattern.cols) / 2;
    let mut field = Field::new(FROG_FIELD, FROG_FIELD);
    field.map(|x, y, _| {
        let inside =
            (top..top + pattern.rows).contains(&y) && (left..left + pattern.cols).contains(&x);
        if inside {
            pattern.values[(y - top) * pattern.cols + (x - left)]
        } else {
            0.0
        }
    });
    let (mut lenia, genesis, persistence) = s613();
    for _ in 0..WARMUP_STEPS {
        lenia.step_glaberish(&mut field, genesis, persistence, 1.0, 1.0);
    }
    let values = snapshot(&field);
    let active = values
        .iter()
        .filter(|v| **v > NEGLIGIBLE_CELL_VALUE)
        .count();
    let mut samples = Vec::with_capacity(FROG_REPETITIONS);
    for repetition in 0..DISCARDED_REPETITIONS + FROG_REPETITIONS {
        restore(&mut field, &values, FROG_FIELD);
        let started = Instant::now();
        lenia.step_glaberish(&mut field, genesis, persistence, 1.0, 1.0);
        let elapsed = started.elapsed().as_secs_f32() * 1e6;
        if repetition >= DISCARDED_REPETITIONS {
            samples.push(elapsed);
        }
    }
    (median_microseconds(samples), active)
}

/// 生物を落ち着かせてから場を固定するまでのステップ数。
const WARMUP_STEPS: u32 = 300;
/// 1構成あたりの計測回数。中央値を取る。
const REPETITIONS: usize = 1_000;
/// 計測前に捨てる回数(キャッシュや分岐予測を落ち着かせる)。
const DISCARDED_REPETITIONS: usize = 50;
/// 疎な畳み込みで寄与元とみなす値(`lenia.rs` の `NEGLIGIBLE_CELL_VALUE` と同じ)。
const NEGLIGIBLE_CELL_VALUE: f32 = 1e-5;
/// M5Stack CoreS3 で実測した、O2u・43×32・1本の `body.step()`(opt-level 3)。
const M5STACK_SINGLE_KERNEL_MS: f32 = 62.0;

/// (構成の説明, カーネル本数)。
const LAYOUTS: [(&str, usize); 6] = [
    ("1ch(模擬)", 1),
    ("2ch・自己1本・相互なし", 2),
    ("2ch・自己1本・相互1本", 4),
    ("2ch・自己2本・相互1本", 6),
    ("3ch・自己1本・相互1本", 9),
    ("3ch・自己3本・相互1本", 15),
];

const FIELDS: [(usize, usize); 2] = [(43, 32), (32, 32)];

fn snapshot(field: &Field) -> Vec<f32> {
    let view = field.view();
    let mut values = Vec::with_capacity(view.width() * view.height());
    for y in 0..view.height() {
        for x in 0..view.width() {
            values.push(view.get(x, y));
        }
    }
    values
}

fn restore(field: &mut Field, values: &[f32], width: usize) {
    field.map(|x, y, _| values[y * width + x]);
}

fn median_microseconds(mut samples: Vec<f32>) -> f32 {
    samples.sort_by(f32::total_cmp);
    samples[samples.len() / 2]
}

/// 生物を動かして落ち着かせた場の値。
fn settled_values(animal: &Animal, width: usize, height: usize) -> Vec<f32> {
    let mut lenia = Lenia::new(animal.params.clone());
    let mut field = Field::new(width, height);
    field.place_centered(&animal.pattern);
    for _ in 0..WARMUP_STEPS {
        lenia.step(&mut field, 1.0);
    }
    snapshot(&field)
}

/// いまの体と同じ1本の1ステップ(場を直接進める)。
fn time_direct(animal: &Animal, values: &[f32], width: usize, height: usize) -> f32 {
    let mut lenia = Lenia::new(animal.params.clone());
    let mut field = Field::new(width, height);
    let mut samples = Vec::with_capacity(REPETITIONS);
    for repetition in 0..DISCARDED_REPETITIONS + REPETITIONS {
        restore(&mut field, values, width);
        let started = Instant::now();
        lenia.step(&mut field, 1.0);
        let elapsed = started.elapsed().as_secs_f32() * 1e6;
        if repetition >= DISCARDED_REPETITIONS {
            samples.push(elapsed);
        }
    }
    median_microseconds(samples)
}

/// `kernels` 本ぶんの1ステップ。カーネルごとに別の `Lenia`(タップと作業領域)と、元の
/// チャンネルの複製を持ち、複製は計測の外で作り直す。
fn time_kernels(
    animal: &Animal,
    values: &[f32],
    width: usize,
    height: usize,
    kernels: usize,
) -> f32 {
    let mut lenias: Vec<Lenia> = (0..kernels)
        .map(|_| Lenia::new(animal.params.clone()))
        .collect();
    let mut sources: Vec<Field> = (0..kernels).map(|_| Field::new(width, height)).collect();
    let mut samples = Vec::with_capacity(REPETITIONS);
    for repetition in 0..DISCARDED_REPETITIONS + REPETITIONS {
        for source in &mut sources {
            restore(source, values, width);
        }
        let started = Instant::now();
        for (lenia, source) in lenias.iter_mut().zip(&mut sources) {
            lenia.step(source, 1.0);
        }
        let elapsed = started.elapsed().as_secs_f32() * 1e6;
        if repetition >= DISCARDED_REPETITIONS {
            samples.push(elapsed);
        }
    }
    median_microseconds(samples)
}

fn main() {
    let started = Instant::now();
    let animals: Vec<Animal> = vmc_pet_body::list_animals()
        .unwrap()
        .into_iter()
        .map(|(code, _)| load_animal(&code).unwrap())
        .collect();

    println!(
        "1ステップの時間(PC、中央値、{REPETITIONS} 回)。カーネルの半径は生物の R にそろえた上限"
    );
    // O2u・43×32 のいまの1本の時間。カエルとの比に使う
    let mut o2u_m5stack_field = None;
    for (width, height) in FIELDS {
        println!();
        println!("==== 場 {width}×{height} ====");
        for animal in &animals {
            let values = settled_values(animal, width, height);
            let active = values
                .iter()
                .filter(|v| **v > NEGLIGIBLE_CELL_VALUE)
                .count();
            let direct = time_direct(animal, &values, width, height);
            if (width, height) == (43, 32) && animal.code == "O2u" {
                o2u_m5stack_field = Some(direct);
            }
            println!(
                "  {} (R = {}、生きているセル {active}/{}): いまの1本 {direct:.1} µs",
                animal.code,
                animal.params.radius,
                width * height
            );
            for (label, kernels) in LAYOUTS {
                let elapsed = time_kernels(animal, &values, width, height, kernels);
                let ratio = elapsed / direct;
                let m5stack = if (width, height) == (43, 32) && animal.code == "O2u" {
                    format!(
                        "  → M5Stack 見積もり {:.0} ms/step",
                        M5STACK_SINGLE_KERNEL_MS * ratio
                    )
                } else {
                    String::new()
                };
                println!(
                    "    {label:24} {kernels:>2} 本: {elapsed:8.1} µs(いまの {ratio:5.2} 倍){m5stack}"
                );
            }
        }
    }
    if let (Some(directory), Some(o2u)) = (std::env::args().nth(1), o2u_m5stack_field) {
        println!();
        println!(
            "==== 比較: s613 のカエル(Glaberish、R = 31、場 {FROG_FIELD}×{FROG_FIELD}、中央値 {FROG_REPETITIONS} 回) ===="
        );
        for name in ["s613_s613_frog000", "frog000"] {
            let (elapsed, active) = time_frog(Path::new(&directory), name);
            let ratio = elapsed / o2u;
            println!(
                "  {name:20} 生きているセル {active}/{}: {elapsed:8.1} µs(O2u・43×32 の1本の {ratio:5.2} 倍)  → M5Stack 見積もり {:.0} ms/step",
                FROG_FIELD * FROG_FIELD,
                M5STACK_SINGLE_KERNEL_MS * ratio
            );
        }
    }
    println!();
    println!("合計 {:.0} 秒", started.elapsed().as_secs_f32());
}
