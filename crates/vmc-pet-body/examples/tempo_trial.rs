//! 【スパイク】粒子の体(1091)のテンポ(気分 → 体の時間の進み方)を測る(issue #6)。
//!
//! Lenia の体では、がっかりしているほど体の時間がゆっくり進む(`Vitality` の tempo、
//! ×0.5〜×1.6 が実測で崩壊のない範囲)。粒子の体にも同じ表情を繋ぐ前に、テンポを
//! どの範囲で変えても塊が保つかを確かめる。
//!
//! テンポの効かせ方は、本実装と同じ `ParticleWorld::step_at_tempo`(位置への反映だけ
//! テンポ倍。速度の更新・力・摩擦・誘い・壁はそのまま)を使う。
//!
//! 測るもの:
//! 1. 継続性: テンポ ×0.25 / ×0.5 / ×0.6 / ×1.0 / ×1.6 / ×3.0 を 20000 ステップ、
//!    塊が保ち・動き続けるか(box16_trial と同じ計測)
//! 2. テンポ × 弱った体(力の倍率 ×0.6 = エネルギー 0 相当)の組
//! 3. 連打崩し + 誘いの戻りが、テンポでどう変わるか

use vmc_pet_body::particles::{ParticleParams, ParticleRng, ParticleWorld};

const BOX: f32 = 16.0;
const SAMPLE_EVERY: u32 = 15;
const VERIFY_STEPS: u32 = 20_000;
const WARMUP_STEPS: u32 = 1500;
const BURST_SECONDS: (u32, u32) = (1, 4);
const BURST_CLICKS_PER_SECOND: (u32, u32) = (1, 4);
const BURST_RADIUS_CELLS: f32 = 3.2;
const RECOVERY_WINDOW: u32 = 3000;
const LURE_STRENGTH: f32 = 0.13;

fn main() {
    println!("=== 継続性: テンポ × 力の倍率 (20000 ステップ、箱 {BOX}) ===");
    for tempo in [0.25, 0.5, 0.6, 1.0, 1.6, 3.0] {
        for force_scale in [1.0f32, 0.6] {
            for trial in 0..4u64 {
                let mut world =
                    ParticleWorld::new(ParticleParams::from_seed(1091), BOX, 100 + trial);
                world.force_scale = force_scale;
                let mut min_cohesion = 1.0f32;
                let mut coverage = [false; 16];
                let mut speed_total = 0.0f32;
                let mut samples = 0u32;
                let mut previous: Option<(f32, f32)> = None;
                for step in 1..=VERIFY_STEPS {
                    world.step_at_tempo(tempo);
                    if step <= WARMUP_STEPS || step % SAMPLE_EVERY != 0 {
                        continue;
                    }
                    let members = world.largest_cluster();
                    let cohesion = members.len() as f32 / world.x.len() as f32;
                    min_cohesion = min_cohesion.min(cohesion);
                    let count = members.len() as f32;
                    let cx = members.iter().map(|&i| world.x[i]).sum::<f32>() / count;
                    let cy = members.iter().map(|&i| world.y[i]).sum::<f32>() / count;
                    if let Some((px, py)) = previous {
                        speed_total +=
                            ((cx - px).powi(2) + (cy - py).powi(2)).sqrt() / SAMPLE_EVERY as f32;
                    }
                    previous = Some((cx, cy));
                    let cell = |v: f32| ((v / BOX * 4.0) as usize).min(3);
                    coverage[cell(cy) * 4 + cell(cx)] = true;
                    samples += 1;
                }
                let samples = samples as f32;
                let visited = coverage.iter().filter(|&&v| v).count();
                println!(
                    "tempo x{tempo:.2} force x{force_scale:.1} trial {trial}: min_cohesion {:.3} mean_speed {:.3} coverage {visited}/16",
                    min_cohesion,
                    speed_total / samples,
                );
            }
        }
    }

    println!("\n=== 連打崩し + 誘い、テンポごとの戻り (32 通りの崩し方) ===");
    for tempo in [0.6f32, 1.0, 1.6] {
        let mut recovered_count = 0u32;
        let mut stray_seconds = Vec::new();
        for burst_seed in 1000..1032u64 {
            let trial = burst_seed % 16;
            let mut world =
                ParticleWorld::new(ParticleParams::from_seed(1091), BOX, 100 + trial);
            let mut rng = ParticleRng::new(burst_seed);
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
                        world.step_at_tempo(tempo);
                    }
                }
            }
            world.lure_radius = BOX;
            let mut strays = 0u32;
            for step in 1..=RECOVERY_WINDOW {
                if step % SAMPLE_EVERY == 1 {
                    let observed = world.observe();
                    world.lure = if observed.strays >= 1 {
                        Some((observed.center.0, observed.center.1, LURE_STRENGTH))
                    } else {
                        None
                    };
                }
                world.step_at_tempo(tempo);
                if step % SAMPLE_EVERY == 0 {
                    if world.observe().strays > 0 {
                        strays += 1;
                    } else {
                        recovered_count += 1;
                        break;
                    }
                }
            }
            stray_seconds.push(strays);
        }
        let mean = stray_seconds.iter().sum::<u32>() as f32 / stray_seconds.len() as f32;
        println!(
            "tempo x{tempo:.1}: 戻れない {}/32, はぐれていた秒 平均 {mean:.1}",
            32 - recovered_count,
        );
    }
}
