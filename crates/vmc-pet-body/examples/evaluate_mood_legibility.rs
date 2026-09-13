//! 「元気/待っている/がっかり」を、外から見える振る舞いで見分けられるかを測る。
//! docs/DESIGN.md「元気/待っている/がっかり」参照。
//!
//! 3つの状態の組ごとに legibility(振る舞いの特徴の相対差)を求める。どれか1組でも
//! 0 に近ければ、その2つは見た目では見分けられない。条件の作り方は
//! `fitness::mood_trajectory` にある。

use vmc_pet_body::fitness::{legibility, mood_trajectory, Mood};

const FIELD_SIZE: usize = 32;
const EVAL_STEPS: u32 = 900;

fn main() {
    println!("状態の組ごとの legibility(0に近い=見た目で見分けられない)");
    println!();
    for (code, name) in vmc_pet_body::list_animals().unwrap() {
        let trajectories: Vec<_> = Mood::ALL
            .iter()
            .map(|mood| (*mood, mood_trajectory(&code, FIELD_SIZE, EVAL_STEPS, *mood)))
            .collect();

        println!("{code:6} {name}");
        for (mood, trajectory) in &trajectories {
            let signature = trajectory.signature(FIELD_SIZE, FIELD_SIZE);
            println!(
                "  {:10} speed={:.4} mass_dev={:.4}",
                mood.label(),
                signature.mean_speed,
                signature.mass_deviation
            );
        }
        let mut weakest = f32::MAX;
        for first in 0..trajectories.len() {
            for second in (first + 1)..trajectories.len() {
                let score = legibility(&trajectories[first].1, &trajectories[second].1, FIELD_SIZE, FIELD_SIZE);
                weakest = weakest.min(score);
                println!(
                    "  {} ⇔ {}: {score:.4}",
                    trajectories[first].0.label(),
                    trajectories[second].0.label()
                );
            }
        }
        println!("  いちばん見分けにくい組: {weakest:.4}");
        println!();
    }
}
