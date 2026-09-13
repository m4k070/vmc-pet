//! 「元気/待っている/がっかり」を、外から見える振る舞いで見分けられるかを測る。
//! docs/DESIGN.md「元気/待っている/がっかり」参照。
//!
//! 3つの状態の組ごとに、動き(legibility: 速さと脈動)と色(体の平均の色づき具合の差)
//! を求め、どちらか大きい方を「見分けやすさ」とする(`fitness::distinguishability`)。
//! どれか1組でも 0 に近ければ、その2つは見た目では見分けられない。条件の作り方は
//! `fitness::mood_trajectory` にある。
//!
//! 状態ごとに、速さ・脈動では測れない動きの質(むら: 止まったり動いたり、
//! まっすぐさ: 行ったり来たりしないか)も並べる。これらは見分けやすさには入れていない
//! (docs/DESIGN.md「身じろぎの届け方(リズム)を試した」)。

use vmc_pet_body::fitness::{distinguishability, legibility, mood_trajectory};
use vmc_pet_body::MoodState;

const FIELD_SIZE: usize = 32;
const EVAL_STEPS: u32 = 900;

/// 動きの質を見る区切り。人の目が動きのむらを感じ取れる1秒(15ステップ)。
const WINDOW_STEPS: usize = 15;

fn main() {
    println!("状態の組ごとの見分けやすさ(0に近い=見た目で見分けられない)");
    println!();
    for (code, name) in vmc_pet_body::list_animals().unwrap() {
        let trajectories: Vec<_> = MoodState::ALL
            .iter()
            .map(|state| {
                (
                    *state,
                    mood_trajectory(&code, FIELD_SIZE, EVAL_STEPS, *state),
                )
            })
            .collect();

        println!("{code:6} {name}");
        for (state, trajectory) in &trajectories {
            let signature = trajectory.signature(FIELD_SIZE, FIELD_SIZE);
            println!(
                "  {:10} speed={:.4} mass_dev={:.4} tint={:.3} unevenness={:.4} straightness={:.4}",
                state.label(),
                signature.mean_speed,
                signature.mass_deviation,
                trajectory.mean_tint(),
                trajectory.speed_unevenness(FIELD_SIZE, FIELD_SIZE, WINDOW_STEPS),
                trajectory.path_straightness(FIELD_SIZE, FIELD_SIZE, WINDOW_STEPS)
            );
        }
        let mut weakest = f32::MAX;
        for first in 0..trajectories.len() {
            for second in (first + 1)..trajectories.len() {
                let (a, b) = (&trajectories[first].1, &trajectories[second].1);
                let motion = legibility(a, b, FIELD_SIZE, FIELD_SIZE);
                let colour = (a.mean_tint() - b.mean_tint()).abs();
                let score = distinguishability(a, b, FIELD_SIZE, FIELD_SIZE);
                weakest = weakest.min(score);
                println!(
                    "  {} ⇔ {}: 見分けやすさ {score:.3}(動き {motion:.3} / 色 {colour:.3})",
                    trajectories[first].0.label(),
                    trajectories[second].0.label()
                );
            }
        }
        println!("  いちばん見分けにくい組: {weakest:.3}");
        println!();
    }
}
