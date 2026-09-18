//! 【スパイク】粒子の体(1091)を 16 セルの箱(zoom 2 表示に相当)で動かすと保つか。
//!
//! 身長の壁(issue #4「表示が小さい」): 32 セルの箱で 1091 の塊は広がり約 2 セルで、
//! 32×32 のドットでは小さすぎる。表示の拡大を物理(zoom = 箱を狭める)でやるなら、
//! 箱 16 セルで探索・試験と同じ性質が保たれるかを先に確かめる。
//!
//! 測るもの(いずれも 32 セル箱の実測と比べる):
//! 1. 継続性: 4 通りの初期配置で 20000 ステップ、塊が 9 割以上を保つか・動き回るか
//! 2. 連打崩し(particle_trial の burst_episode と同じ形)+ 誘いコントローラ
//!    (はぐれ 1 個以上・強さ 0.13・距離 39)で戻るまでの秒
//! 3. ふだんの動きの速さと塊の広がり

use vmc_pet_body::particles::{ParticleParams, ParticleRng, ParticleWorld};

/// 箱の一辺。zoom 2 で 32×32 のドットいっぱいに見える大きさ。
const BOX: f32 = 16.0;
const SAMPLE_EVERY: u32 = 15;
const VERIFY_STEPS: u32 = 20_000;
const WARMUP_STEPS: u32 = 1500;
/// 連打崩しの形(particle_trial.rs の BURST_* と同じ)。
const BURST_SECONDS: (u32, u32) = (1, 4);
const BURST_CLICKS_PER_SECOND: (u32, u32) = (1, 4);
const BURST_RADIUS_CELLS: f32 = 3.2;
const RECOVERY_WINDOW: u32 = 3000;
const LURE_STRENGTH: f32 = 0.13;
const LURE_RADIUS: f32 = 39.0;

fn main() {
    println!("=== 継続性 (20000 ステップ、箱 {BOX}) ===");
    for trial in 0..4u64 {
        let mut world =
            ParticleWorld::new(ParticleParams::from_seed(1091), BOX, 100 + trial);
        let mut min_cohesion = 1.0f32;
        let mut coverage = [false; 16];
        let mut speed_total = 0.0f32;
        let mut radius_total = 0.0f32;
        let mut samples = 0u32;
        let mut previous: Option<(f32, f32)> = None;
        for step in 1..=VERIFY_STEPS {
            world.step();
            if step <= WARMUP_STEPS || step % SAMPLE_EVERY != 0 {
                continue;
            }
            let members = world.largest_cluster();
            let cohesion = members.len() as f32 / world.x.len() as f32;
            min_cohesion = min_cohesion.min(cohesion);
            let count = members.len() as f32;
            let cx = members.iter().map(|&i| world.x[i]).sum::<f32>() / count;
            let cy = members.iter().map(|&i| world.y[i]).sum::<f32>() / count;
            let radius = (members
                .iter()
                .map(|&i| (world.x[i] - cx).powi(2) + (world.y[i] - cy).powi(2))
                .sum::<f32>()
                / count)
                .sqrt();
            if let Some((px, py)) = previous {
                speed_total += ((cx - px).powi(2) + (cy - py).powi(2)).sqrt()
                    / SAMPLE_EVERY as f32;
            }
            previous = Some((cx, cy));
            radius_total += radius;
            let cell = |v: f32| ((v / BOX * 4.0) as usize).min(3);
            coverage[cell(cy) * 4 + cell(cx)] = true;
            samples += 1;
        }
        let samples = samples as f32;
        let coverage = coverage.iter().filter(|&&v| v).count();
        println!(
            "trial {trial}: min_cohesion {:.3} mean_speed {:.3} mean_radius {:.2} coverage {coverage}/16",
            min_cohesion,
            speed_total / samples,
            radius_total / samples,
        );
    }

    println!("\n=== 連打崩し + 誘い (32 通りの崩し方、箱 {BOX}) ===");
    // ふだんの速さ(吸い寄せの代償の基準)を、崩していない体で先に出す
    let mut idle_speed = 0.0f32;
    for trial in 0..16u64 {
        let mut world =
            ParticleWorld::new(ParticleParams::from_seed(1091), BOX, 100 + trial);
        for step in 1..=1800 {
            world.step();
            if step % SAMPLE_EVERY == 0 {
                idle_speed += world.mean_speed();
            }
        }
    }
    let idle_speed = idle_speed / (16.0 * 120.0);
    println!("ふだんの平均の速さ: {idle_speed:.3} セル/ステップ");

    const TRIALS: u64 = 16;
    let mut stray_seconds = Vec::new();
    let mut recovery_seconds = Vec::new();
    let mut broke_count = 0u32;
    for burst_seed in 1000..1032u64 {
        let trial = burst_seed % TRIALS;
        let mut world =
            ParticleWorld::new(ParticleParams::from_seed(1091), BOX, 100 + trial);
        let mut rng = ParticleRng::new(burst_seed);
        // 連打: 1 秒に 1〜4 回、1〜4 秒続く。塊の重心から 3.2 セル以内を弾く
        let seconds =
            rng.range(BURST_SECONDS.0 as f32, BURST_SECONDS.1 as f32 + 0.99) as u32;
        for _ in 0..seconds {
            let clicks = rng.range(
                BURST_CLICKS_PER_SECOND.0 as f32,
                BURST_CLICKS_PER_SECOND.1 as f32 + 0.99,
            ) as u32;
            for _ in 0..clicks {
                let observed = world.observe();
                let angle = rng.range(0.0, std::f32::consts::TAU);
                let distance = BURST_RADIUS_CELLS * rng.unit().sqrt();
                world.poke(
                    observed.center.0 + distance * angle.cos(),
                    observed.center.1 + distance * angle.sin(),
                    4.5,
                    1.0,
                );
                for _ in 0..(SAMPLE_EVERY / clicks.max(1)) {
                    world.step();
                }
            }
        }
        // 誘いコントローラ: はぐれた粒子が 1 個以上で塊の重心へ誘う(1 秒ごとに測り直す)
        world.lure_radius = LURE_RADIUS.min(BOX - 2.0);
        let mut strays_seconds = 0u32;
        let mut recovered = false;
        let mut max_excess_units = 0.0f32;
        for step in 1..=RECOVERY_WINDOW {
            if step % SAMPLE_EVERY == 1 {
                let observed = world.observe();
                world.lure = if observed.strays >= 1 {
                    Some((
                        observed.center.0,
                        observed.center.1,
                        LURE_STRENGTH,
                    ))
                } else {
                    None
                };
            }
            world.step();
            if world.lure.is_some() {
                let excess = (world.mean_speed() - idle_speed).max(0.0) / idle_speed / 15.0;
                max_excess_units = max_excess_units.max(excess);
            }
            if step % SAMPLE_EVERY == 0 {
                let observed = world.observe();
                if observed.strays > 0 {
                    strays_seconds += 1;
                } else if !recovered {
                    recovery_seconds.push(step as f32 / 15.0);
                    recovered = true;
                }
            }
        }
        if !recovered {
            recovery_seconds.push(RECOVERY_WINDOW as f32 / 15.0);
            broke_count += 1;
        }
        stray_seconds.push(strays_seconds);
        if burst_seed < 1008 {
            println!(
                "burst #{burst_seed}: stray_seconds {strays_seconds} recovery {:.1}s max_excess {max_excess_units:.4}",
                recovery_seconds.last().copied().unwrap_or(f32::NAN),
            );
        }
    }
    let mean = |values: &[u32]| values.iter().sum::<u32>() as f32 / values.len() as f32;
    println!(
        "はぐれていた秒: 平均 {:.1} / 32 通り、戻れない {broke_count}/32",
        mean(&stray_seconds),
    );
    let recovery_mean = recovery_seconds.iter().sum::<f32>() / recovery_seconds.len() as f32;
    println!("戻るまでの秒: 平均 {recovery_mean:.1}");
}
