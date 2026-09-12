//! 自律コントローラ(や、将来の学習可能なコントローラ)を比較するための
//! 評価ツール。docs/DESIGN.md「学習可能なコントローラへ向けた代理指標」参照。
//!
//! 「コントローラなし(素のLeniaBody)」と「今のコントローラ」を、
//! 同じ長さの軌跡・同じ評価式(fitness::Trajectory)で比べる。

use vmc_pet_body::fitness::Trajectory;
use vmc_pet_body::Pet;

const FIELD_SIZE: usize = 32;
const WARMUP_STEPS: u32 = 60;
const EVAL_STEPS: u32 = 900;

fn main() {
    let mut pet = Pet::load("O2u", FIELD_SIZE, FIELD_SIZE).unwrap();
    for _ in 0..WARMUP_STEPS {
        pet.step();
    }
    let trajectory = Trajectory::record(&mut pet, EVAL_STEPS);
    let score = trajectory.fitness(FIELD_SIZE, FIELD_SIZE);
    println!(
        "current controller: {EVAL_STEPS} steps, fitness={score:.4} (higher = more movement, {} = collapsed)",
        vmc_pet_body::fitness::COLLAPSE_PENALTY
    );
}
