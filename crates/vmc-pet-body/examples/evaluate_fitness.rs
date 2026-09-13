//! 自律コントローラ(や、将来の学習可能なコントローラ)を比較するための
//! 評価ツール。docs/DESIGN.md「学習可能なコントローラへ向けた代理指標」参照。
//!
//! **旧い道具**: ここで使う `fitness`(重心の移動量=動きの多さ)は、目的関数としては
//! 採らないと決めたもの。崖のふちへ候補を追い込むだけで、経験に関わる要素も入って
//! いなかった(docs/DESIGN.md「世話のされ方が振る舞いに現れているか(legibility)」)。
//! 判断の記録にある実験を再現するために残してある。いまの評価には
//! `examples/evaluate_legibility.rs` と `examples/evaluate_mood_legibility.rs` を使う。
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
