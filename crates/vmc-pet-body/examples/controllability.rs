//! 【実験】コントローラのパラメータは、報酬の候補をどれだけ動かせるか(issue #3 の手順1)。
//!
//! issue #3 は、場の総量・活動度を一定範囲に保つことを報酬にして、コントローラのパラメータ
//! (`ControllerParams`)をオンラインの進化戦略で調整する案。学習を実装する前に、そもそもパラメータを
//! 変えると報酬の候補が変わるのかを測る。変わる幅が、同じパラメータで出発点が違うだけのばらつき
//! (1回の評価の雑音)に埋もれるなら、学習の信号にならない。
//!
//! - 生物: 同梱の4種。場はペットと同じ 32×32。ペット(`Pet`)をそのまま動かす
//! - 体の状態: 世話され続けている(エネルギーを 1.0 に固定)/放置された(触らずに尽きさせる)。
//!   `fitness::cared_for_trajectory` / `neglected_trajectory` と同じ条件
//! - パラメータ: コントローラなし(突く量 0)、いまの既定値、既定値の近く(各値 ±20% の一様乱数)12組、
//!   探索で使った広い範囲(`search_controller_params.rs` の BOUNDS、弱ったときの係数は 0〜1)12組
//! - 出発点: 助走の長さを 150 ステップずつずらした 8 通り
//! - 評価の窓: 900 ステップ(60 秒)。オンラインで1候補を評価する長さの目安
//! - 報酬の候補: 総量の平均・総量の標準偏差(脈動)・活動度(1ステップのセルの変化の絶対値の平均)の
//!   平均と標準偏差・重心の速さ
//!
//! 比べる量:
//!
//! - パラメータによる差: 出発点で平均した値の、パラメータ組のあいだの標準偏差
//! - 1回の評価の雑音: 同じパラメータで出発点だけ違うときの標準偏差(パラメータ組で平均)
//! - 世話と放置の差: 既定値での、世話された体と放置された体の平均の差
//!
//! 使い方: `cargo run --release -p vmc-pet-body --example controllability`。ペット本体の振る舞いには触れない。

use std::sync::Mutex;
use std::thread;
use std::time::Instant;

use vmc_pet_body::{list_animals, load_animal, ControllerParams, Pet, PetMemory};

const FIELD_SIZE: usize = 32;
const CELLS: usize = FIELD_SIZE * FIELD_SIZE;
const EVAL_STEPS: u32 = 900;
const STARTS: u32 = 8;
const START_SPACING: u32 = 150;
/// `fitness.rs` の助走と同じ長さ。
const CARED_WARMUP_STEPS: u32 = 60;
const NEGLECTED_WARMUP_STEPS: u32 = 3000;
const PARAMS_PER_BOX: usize = 12;
/// 既定値の近くとみなす幅(各値の ±割合)。
const NEAR_SPREAD: f32 = 0.2;

const METRICS: [&str; 5] = [
    "総量の平均",
    "総量の標準偏差",
    "活動度の平均",
    "活動度の標準偏差",
    "重心の速さ",
];

#[derive(Clone, Copy, PartialEq)]
enum BodyState {
    CaredFor,
    Neglected,
}

impl BodyState {
    fn label(self) -> &'static str {
        match self {
            Self::CaredFor => "世話されている",
            Self::Neglected => "放置された",
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum ParamsKind {
    NoController,
    Default,
    Near,
    Wide,
}

/// 固定シードの疑似乱数(xorshift64。`search_controller_params.rs` と同じ)。
struct Rng(u64);

impl Rng {
    fn unit(&mut self) -> f32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        (x >> 11) as f32 / (1u64 << 53) as f32
    }

    fn range(&mut self, low: f32, high: f32) -> f32 {
        low + (high - low) * self.unit()
    }
}

fn near_params(rng: &mut Rng) -> ControllerParams {
    let base = ControllerParams::default();
    let mut scaled = |value: f32| value * rng.range(1.0 - NEAR_SPREAD, 1.0 + NEAR_SPREAD);
    ControllerParams {
        evaluate_every_steps: scaled(base.evaluate_every_steps as f32).round().max(1.0) as u32,
        imbalance_threshold: scaled(base.imbalance_threshold),
        nudge_radius_cells: scaled(base.nudge_radius_cells),
        nudge_amount: scaled(base.nudge_amount),
        nudge_offset_cells: scaled(base.nudge_offset_cells),
        nudge_amount_when_depleted: scaled(base.nudge_amount_when_depleted).min(1.0),
    }
}

fn wide_params(rng: &mut Rng) -> ControllerParams {
    ControllerParams {
        evaluate_every_steps: rng.range(10.0, 60.0).round() as u32,
        imbalance_threshold: rng.range(0.15, 0.5),
        nudge_radius_cells: rng.range(2.0, 6.0),
        nudge_amount: rng.range(0.02, 0.30),
        nudge_offset_cells: rng.range(1.0, 6.0),
        nudge_amount_when_depleted: rng.range(0.0, 1.0),
    }
}

fn parameter_sets() -> Vec<(ParamsKind, ControllerParams)> {
    let mut rng = Rng(0x5eed_c0de_1234_5678);
    let mut sets = vec![
        (
            ParamsKind::NoController,
            ControllerParams {
                nudge_amount: 0.0,
                ..ControllerParams::default()
            },
        ),
        (ParamsKind::Default, ControllerParams::default()),
    ];
    for _ in 0..PARAMS_PER_BOX {
        sets.push((ParamsKind::Near, near_params(&mut rng)));
    }
    for _ in 0..PARAMS_PER_BOX {
        sets.push((ParamsKind::Wide, wide_params(&mut rng)));
    }
    sets
}

fn mean(values: &[f32]) -> f32 {
    values.iter().sum::<f32>() / values.len().max(1) as f32
}

fn standard_deviation(values: &[f32]) -> f32 {
    let m = mean(values);
    (values.iter().map(|v| (v - m) * (v - m)).sum::<f32>() / values.len().max(1) as f32).sqrt()
}

fn toroidal_offset(offset: f32) -> f32 {
    let size = FIELD_SIZE as f32;
    let wrapped = offset.rem_euclid(size);
    if wrapped > size / 2.0 {
        wrapped - size
    } else {
        wrapped
    }
}

/// 1回の評価の報酬の候補。崩壊したら `None`。
fn evaluate(
    code: &str,
    state: BodyState,
    params: ControllerParams,
    start: u32,
) -> Option<[f32; 5]> {
    let animal = load_animal(code).unwrap();
    let mut pet = Pet::with_controller_params(animal, FIELD_SIZE, FIELD_SIZE, params);
    let hold_energy = |pet: &mut Pet| {
        if state == BodyState::CaredFor {
            pet.restore(PetMemory::with_energy(1.0), 0.0);
        }
    };
    let warmup = match state {
        BodyState::CaredFor => CARED_WARMUP_STEPS,
        BodyState::Neglected => NEGLECTED_WARMUP_STEPS,
    } + START_SPACING * start;
    for _ in 0..warmup {
        hold_energy(&mut pet);
        if pet.step() {
            return None;
        }
    }

    let mut previous: Vec<f32> = snapshot(&pet);
    let mut previous_centroid = pet.observe().toroidal_centroid();
    let (mut masses, mut activities, mut speeds) = (Vec::new(), Vec::new(), Vec::new());
    for _ in 0..EVAL_STEPS {
        hold_energy(&mut pet);
        if pet.step() {
            return None;
        }
        let current = snapshot(&pet);
        let change: f32 = current
            .iter()
            .zip(&previous)
            .map(|(a, b)| (a - b).abs())
            .sum();
        activities.push(change / CELLS as f32);
        masses.push(pet.mass());
        let centroid = pet.observe().toroidal_centroid();
        if let (Some(a), Some(b)) = (previous_centroid, centroid) {
            let (dx, dy) = (toroidal_offset(b.0 - a.0), toroidal_offset(b.1 - a.1));
            speeds.push((dx * dx + dy * dy).sqrt());
        }
        previous = current;
        previous_centroid = centroid;
    }
    Some([
        mean(&masses),
        standard_deviation(&masses),
        mean(&activities),
        standard_deviation(&activities),
        mean(&speeds),
    ])
}

fn snapshot(pet: &Pet) -> Vec<f32> {
    let view = pet.observe();
    (0..FIELD_SIZE)
        .flat_map(|y| (0..FIELD_SIZE).map(move |x| (x, y)))
        .map(|(x, y)| view.get(x, y))
        .collect()
}

/// パラメータ組ごとの、出発点ごとの値(崩壊した出発点は除く)。
struct Evaluations {
    values: Vec<Vec<[f32; 5]>>,
    collapsed: Vec<u32>,
}

fn run_all(code: &str, state: BodyState, sets: &[(ParamsKind, ControllerParams)]) -> Evaluations {
    let jobs: Vec<(usize, u32)> = (0..sets.len())
        .flat_map(|p| (0..STARTS).map(move |s| (p, s)))
        .collect();
    let jobs = Mutex::new(jobs);
    let results = Mutex::new(Vec::new());
    thread::scope(|scope| {
        for _ in 0..thread::available_parallelism().map_or(4, |n| n.get()) {
            scope.spawn(|| loop {
                let Some((p, s)) = jobs.lock().unwrap().pop() else {
                    break;
                };
                let outcome = evaluate(code, state, sets[p].1, s);
                results.lock().unwrap().push((p, outcome));
            });
        }
    });
    let mut evaluations = Evaluations {
        values: vec![Vec::new(); sets.len()],
        collapsed: vec![0; sets.len()],
    };
    for (p, outcome) in results.into_inner().unwrap() {
        match outcome {
            Some(values) => evaluations.values[p].push(values),
            None => evaluations.collapsed[p] += 1,
        }
    }
    evaluations
}

/// 選んだパラメータ組について、指標ごとに (パラメータによる差, 1回の評価の雑音) を返す。
fn spread_and_noise(evaluations: &Evaluations, chosen: &[usize], metric: usize) -> (f32, f32) {
    let usable: Vec<&Vec<[f32; 5]>> = chosen
        .iter()
        .map(|&p| &evaluations.values[p])
        .filter(|values| values.len() >= 2)
        .collect();
    let means: Vec<f32> = usable
        .iter()
        .map(|values| mean(&values.iter().map(|v| v[metric]).collect::<Vec<_>>()))
        .collect();
    let noises: Vec<f32> = usable
        .iter()
        .map(|values| standard_deviation(&values.iter().map(|v| v[metric]).collect::<Vec<_>>()))
        .collect();
    (standard_deviation(&means), mean(&noises))
}

fn metric_mean(evaluations: &Evaluations, p: usize, metric: usize) -> f32 {
    mean(
        &evaluations.values[p]
            .iter()
            .map(|v| v[metric])
            .collect::<Vec<_>>(),
    )
}

fn main() {
    let started = Instant::now();
    let sets = parameter_sets();
    let indices = |kind: ParamsKind| -> Vec<usize> {
        sets.iter()
            .enumerate()
            .filter(|(_, (k, _))| *k == kind)
            .map(|(i, _)| i)
            .collect()
    };
    let (near, wide) = (indices(ParamsKind::Near), indices(ParamsKind::Wide));
    let (no_controller, default) = (0, 1);
    let near_and_default: Vec<usize> = std::iter::once(default)
        .chain(near.iter().copied())
        .collect();

    println!(
        "場 {FIELD_SIZE}×{FIELD_SIZE}、評価の窓 {EVAL_STEPS} ステップ、出発点 {STARTS} 通り、近く・広い範囲 各 {PARAMS_PER_BOX} 組\n"
    );
    for (code, name) in list_animals().unwrap() {
        let cared = run_all(&code, BodyState::CaredFor, &sets);
        let neglected = run_all(&code, BodyState::Neglected, &sets);
        println!("## {code} ({name})");
        for (state, evaluations) in [
            (BodyState::CaredFor, &cared),
            (BodyState::Neglected, &neglected),
        ] {
            let collapsed_near: u32 = near_and_default
                .iter()
                .map(|&p| evaluations.collapsed[p])
                .sum();
            let collapsed_wide: u32 = wide.iter().map(|&p| evaluations.collapsed[p]).sum();
            println!(
                "\n### {}(崩壊: 既定値と近く {collapsed_near}/{}、広い範囲 {collapsed_wide}/{}、コントローラなし {}/{STARTS})",
                state.label(),
                near_and_default.len() as u32 * STARTS,
                wide.len() as u32 * STARTS,
                evaluations.collapsed[no_controller],
            );
            println!("| 指標 | 既定値 | 雑音(1回の評価) | 近く: 差 | 近く: 差/雑音 | 近く: 差/世話と放置の差 | 広い: 差 | 広い: 差/雑音 | 広い: 差/世話と放置の差 | なし−既定値 | 世話−放置 |");
            println!("|---|---|---|---|---|---|---|---|---|---|---|");
            for (metric, label) in METRICS.iter().enumerate() {
                let (near_spread, near_noise) =
                    spread_and_noise(evaluations, &near_and_default, metric);
                let (wide_spread, wide_noise) = spread_and_noise(evaluations, &wide, metric);
                let noise = (near_noise + wide_noise) / 2.0;
                let default_value = metric_mean(evaluations, default, metric);
                let without = metric_mean(evaluations, no_controller, metric) - default_value;
                let care_gap =
                    metric_mean(&cared, default, metric) - metric_mean(&neglected, default, metric);
                let per_gap = |spread: f32| spread / care_gap.abs().max(1e-9);
                println!(
                    "| {label} | {default_value:.4} | {noise:.4} | {near_spread:.4} | {:.1} | {:.2} | {wide_spread:.4} | {:.1} | {:.2} | {without:+.4} | {care_gap:+.4} |",
                    near_spread / near_noise.max(1e-9),
                    per_gap(near_spread),
                    wide_spread / wide_noise.max(1e-9),
                    per_gap(wide_spread),
                );
            }
            // 広い範囲で、既定値から世話と放置の差の半分以上ずれた組(形そのものが変わった可能性がある)
            let shifted: Vec<String> = wide
                .iter()
                .filter(|&&p| !evaluations.values[p].is_empty())
                .filter(|&&p| {
                    let gap = (metric_mean(&cared, default, 0)
                        - metric_mean(&neglected, default, 0))
                    .abs();
                    (metric_mean(evaluations, p, 0) - metric_mean(evaluations, default, 0)).abs()
                        > gap / 2.0
                })
                .map(|&p| format!("{:?}", sets[p].1))
                .collect();
            if !shifted.is_empty() {
                println!(
                    "\n総量の平均が既定値から世話と放置の差の半分以上ずれた広い範囲の組 {} 個:",
                    shifted.len()
                );
                for params in shifted {
                    println!("- {params}");
                }
            }
        }
        println!();
    }
    println!("所要時間 {:.1} 秒", started.elapsed().as_secs_f32());
}
