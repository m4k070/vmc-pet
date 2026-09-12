//! 自律コントローラのパラメータ(`ControllerParams`)を、`fitness::Trajectory`
//! を使ってランダムサーチする。docs/DESIGN.md「学習可能なコントローラへ向けた
//! 代理指標」参照。
//!
//! 短い評価窓(900ステップ≈60秒)は「動きの多さ」を測るのに使えるが、実際に
//! 崩壊するかどうかの予兆にはならないことが分かっている(fitness.rs参照)。
//! そのため、短い窓の評価で上位に残った候補だけを、長い窓(20000ステップ
//! ≈22分。既存の安全性テストと同じ長さ)で再検証してから報告する。
//!
//! 学習(勾配降下・進化戦略など)はまだしていない。ここではランダムサーチで
//! 「この評価軸のもとで、手で決めた値よりましな組み合わせがあるか」を
//! 確かめるだけにとどめる。

use vmc_pet_body::fitness::{Trajectory, COLLAPSE_PENALTY};
use vmc_pet_body::{ControllerParams, Pet};

const FIELD_SIZE: usize = 32;
const WARMUP_STEPS: u32 = 60;
const SHORT_EVAL_STEPS: u32 = 900;
const LONG_EVAL_STEPS: u32 = 20_000;
const TRIALS: usize = 300;
const TOP_N: usize = 5;

/// 外部クレートに頼らない、この探索専用の決定的な疑似乱数(xorshift64)。
/// 固定シードにしてあるので、実行するたびに同じ候補列を試す。
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn range_f32(&mut self, low: f32, high: f32) -> f32 {
        let fraction = (self.next_u64() >> 11) as f32 / (1u64 << 53) as f32;
        low + fraction * (high - low)
    }

    fn range_u32(&mut self, low: u32, high: u32) -> u32 {
        low + (self.next_u64() % (high - low + 1) as u64) as u32
    }
}

/// 探索範囲。現在の採用値(`ControllerParams::default()`)を中心に、
/// 明らかに極端すぎる値(反応が無くなる・逆に暴れすぎる)は除いてある。
fn random_params(rng: &mut Rng) -> ControllerParams {
    ControllerParams {
        evaluate_every_steps: rng.range_u32(10, 60),
        imbalance_threshold: rng.range_f32(0.15, 0.5),
        nudge_radius_cells: rng.range_f32(2.0, 6.0),
        nudge_amount: rng.range_f32(0.02, 0.30),
        nudge_offset_cells: rng.range_f32(1.0, 6.0),
    }
}

fn evaluate(params: ControllerParams, steps: u32) -> f32 {
    let animal = vmc_pet_body::load_animal("O2u").unwrap();
    let mut pet = Pet::with_controller_params(animal, FIELD_SIZE, FIELD_SIZE, params);
    for _ in 0..WARMUP_STEPS {
        pet.step();
    }
    Trajectory::record(&mut pet, steps).fitness(FIELD_SIZE, FIELD_SIZE)
}

fn main() {
    let baseline_short = evaluate(ControllerParams::default(), SHORT_EVAL_STEPS);
    let baseline_long = evaluate(ControllerParams::default(), LONG_EVAL_STEPS);
    println!(
        "baseline (current defaults): short={baseline_short:.4} long_run={baseline_long:.4} {:?}",
        ControllerParams::default()
    );
    println!();

    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut results: Vec<(ControllerParams, f32)> = Vec::with_capacity(TRIALS);
    for _ in 0..TRIALS {
        let params = random_params(&mut rng);
        let score = evaluate(params, SHORT_EVAL_STEPS);
        results.push((params, score));
    }
    results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

    println!("top {TOP_N} of {TRIALS} random trials (short eval, {SHORT_EVAL_STEPS} steps):");
    for (params, score) in results.iter().take(TOP_N) {
        println!("  score={score:.4} {params:?}");
    }

    println!();
    println!("re-verifying top {TOP_N} with a long run ({LONG_EVAL_STEPS} steps, checks for collapse):");
    for (params, short_score) in results.iter().take(TOP_N) {
        let long_score = evaluate(*params, LONG_EVAL_STEPS);
        let verdict = if long_score == COLLAPSE_PENALTY { "COLLAPSED" } else { "survived" };
        println!("  short={short_score:.4} long_run={long_score:.4} [{verdict}] {params:?}");
    }
}
