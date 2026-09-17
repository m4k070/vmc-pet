//! 【実験】閉じた場を動き回る、非対称な粒子系の体を探す。
//!
//! Lenia の体は、崩壊の崖があり、形の変化が一方通行で、安全な強さで突いても向きを導けなかった
//! (docs/experiments/expression-axes.md、controller-learning.md)。また場の端で値を減らすと生物が
//! 死ぬので、場はトーラスにしてある。そこで、粒子の数が保存され、壁を押し返す力として書ける粒子系で、
//! 閉じた場(端のある箱)を1つの塊のまま動き回る体があるかを探す
//! (docs/experiments/body-candidates.md「閉じた場を動き回る粒子の体を探す」)。
//!
//! 規則は Particle Life 系(Ventrella の Clusters、Mohr の実装の力の形)に倣う。
//!
//! - 粒子は種類 `k` を持つ。距離 r(相互作用の届く距離 `r_max` で割った値)に対する力は、
//!   r < β で全種類共通の反発 `r/β − 1`、β ≤ r < 1 で係数 `a[k_i][k_j]` の山型の引き合い
//! - `a[i][j]` と `a[j][i]` が違ってよい(非対称)。作用と反作用が釣り合わないので、塊が自分で進みうる
//! - 速度は毎ステップ `friction` 倍に減衰し、力を足す。壁の手前 `WALL_MARGIN` セルでは内向きに押し返す
//!
//! 使い方:
//!
//! - `cargo run --release -p vmc-pet-body --example particle_trial -- search <出力先>`:
//!   乱数のパラメータを短い窓でふるい、残った候補を長い窓と別の初期配置で確かめ、合格した候補の姿と
//!   重心の軌跡を PPM で書き出す(約30秒)
//! - `cargo run --release -p vmc-pet-body --example particle_trial -- suite <候補の番号>...`:
//!   候補を、突く・散らす/弱らせて戻す/誘いで導く/1ステップの重さ、の試験にかける(約20秒)
//! - `cargo run --release -p vmc-pet-body --example particle_trial -- lure <候補の番号>...`:
//!   誘いの強さを上げていったとき、導ける度合いと、誘っているあいだ・やめた後の姿を測る
//! - `cargo run --release -p vmc-pet-body --example particle_trial -- regroup <候補の番号>...`:
//!   崩れた体を、手で書いた簡単な行動(塊の重心へ誘う・進む力を弱めて待つ)でまとめ直せるかを測る
//! - `cargo run --release -p vmc-pet-body --example particle_trial -- landscape <候補の番号>...`:
//!   誘いの代償を入れた報酬の地形を、行動のパラメータの格子で測る
//!
//! ペット本体の振る舞いには触れない。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread;
use std::time::Instant;

use vmc_pet_body::particles::{ParticleParams, ParticleRng, ParticleWorld};

/// 場の一辺(セル)。ペットの場と同じ。
const FIELD: f32 = 32.0;
/// ふるいの候補数と窓。
const CANDIDATES: u64 = 3000;
const SCREEN_STEPS: u32 = 4000;
/// 確かめの窓と、別の初期配置の数。
const VERIFY_STEPS: u32 = 20_000;
const VERIFY_SEEDS: u64 = 4;
/// 観測は `SAMPLE_EVERY` ステップごと(15 ステップ = 1 秒)。最初の `WARMUP_STEPS` は数えない。
const SAMPLE_EVERY: u32 = 15;
const WARMUP_STEPS: u32 = 1500;

/// 合格の条件。
/// 最大の塊にいる粒子の割合が、観測のあいだずっとこれ以上。
const COHESION: f32 = 0.9;
/// 重心の速さ(セル/ステップ)の平均がこれ以上。0.05 は 1 秒に 0.75 セル。
const MIN_SPEED: f32 = 0.05;
/// 場を 8×8 に分けた区画のうち、重心が訪れた割合がこれ以上。
const MIN_COVERAGE: f32 = 0.5;
/// 塊の広がり(重心からの距離の二乗平均の平方根)がこれ以下。ペットの場に1つの体として収まる大きさ。
const MAX_RADIUS: f32 = 8.0;

/// 塊の重心と広がり。
fn centroid_and_radius(world: &ParticleWorld, members: &[usize]) -> ((f32, f32), f32) {
    let count = members.len() as f32;
    let cx = members.iter().map(|&i| world.x[i]).sum::<f32>() / count;
    let cy = members.iter().map(|&i| world.y[i]).sum::<f32>() / count;
    let spread = members
        .iter()
        .map(|&i| (world.x[i] - cx).powi(2) + (world.y[i] - cy).powi(2))
        .sum::<f32>()
        / count;
    ((cx, cy), spread.sqrt())
}

/// 1回の走らせの観測。
#[derive(Clone, Debug, Default)]
struct Observation {
    /// 最大の塊にいる粒子の割合の最小。
    min_cohesion: f32,
    mean_speed: f32,
    coverage: f32,
    mean_radius: f32,
    /// 重心が壁から 4 セル以内にいた割合。
    near_wall: f32,
    trail: Vec<(f32, f32)>,
}

impl Observation {
    fn passes(&self) -> bool {
        self.min_cohesion >= COHESION
            && self.mean_speed >= MIN_SPEED
            && self.coverage >= MIN_COVERAGE
            && self.mean_radius <= MAX_RADIUS
    }
}

fn observe(params: &ParticleParams, seed: u64, steps: u32) -> (Observation, ParticleWorld) {
    let mut world = ParticleWorld::new(params.clone(), FIELD, seed);
    let mut observation = Observation {
        min_cohesion: 1.0,
        ..Default::default()
    };
    let mut visited = [false; 64];
    let (mut speed_total, mut radius_total, mut near_wall, mut samples) = (0.0, 0.0, 0, 0);
    let mut previous: Option<(f32, f32)> = None;
    for step in 1..=steps {
        world.step();
        if step <= WARMUP_STEPS || step % SAMPLE_EVERY != 0 {
            continue;
        }
        let members = world.largest_cluster();
        let cohesion = members.len() as f32 / world.x.len() as f32;
        observation.min_cohesion = observation.min_cohesion.min(cohesion);
        let ((cx, cy), radius) = centroid_and_radius(&world, &members);
        if let Some((px, py)) = previous {
            speed_total += ((cx - px).powi(2) + (cy - py).powi(2)).sqrt() / SAMPLE_EVERY as f32;
        }
        previous = Some((cx, cy));
        radius_total += radius;
        let cell = |v: f32| ((v / FIELD * 8.0) as usize).min(7);
        visited[cell(cy) * 8 + cell(cx)] = true;
        if cx.min(cy).min(FIELD - cx).min(FIELD - cy) < 4.0 {
            near_wall += 1;
        }
        observation.trail.push((cx, cy));
        samples += 1;
    }
    let samples = samples.max(1) as f32;
    observation.mean_speed = speed_total / samples;
    observation.mean_radius = radius_total / samples;
    observation.coverage = visited.iter().filter(|&&v| v).count() as f32 / 64.0;
    observation.near_wall = near_wall as f32 / samples;
    (observation, world)
}

/// 姿(粒子を種類で塗り分け)と重心の軌跡を PPM で書く。
fn write_snapshot(path: &Path, world: &ParticleWorld, trail: &[(f32, f32)]) {
    const SCALE: usize = 8;
    let size = FIELD as usize * SCALE;
    let mut pixels = vec![[16u8, 16, 16]; size * size];
    for &(cx, cy) in trail {
        let (px, py) = ((cx * SCALE as f32) as usize, (cy * SCALE as f32) as usize);
        if px < size && py < size {
            pixels[py * size + px] = [90, 90, 90];
        }
    }
    const COLORS: [[u8; 3]; 3] = [[255, 90, 70], [80, 220, 120], [90, 150, 255]];
    for i in 0..world.x.len() {
        let (px, py) = (
            (world.x[i] * SCALE as f32) as isize,
            (world.y[i] * SCALE as f32) as isize,
        );
        for dy in -2..=2 {
            for dx in -2..=2 {
                let (x, y) = (px + dx, py + dy);
                if x >= 0 && y >= 0 && (x as usize) < size && (y as usize) < size {
                    pixels[y as usize * size + x as usize] = COLORS[world.kind[i] % 3];
                }
            }
        }
    }
    let mut data = format!("P6\n{size} {size}\n255\n").into_bytes();
    for pixel in pixels {
        data.extend_from_slice(&pixel);
    }
    fs::write(path, data).expect("画像を書けない");
}

fn parallel_map<T: Send, R: Send>(items: Vec<T>, work: impl Fn(T) -> R + Sync) -> Vec<R> {
    let items = Mutex::new(items.into_iter().enumerate().collect::<Vec<_>>());
    let results = Mutex::new(Vec::new());
    thread::scope(|scope| {
        for _ in 0..thread::available_parallelism().map_or(4, |n| n.get()) {
            scope.spawn(|| loop {
                let Some((index, item)) = items.lock().unwrap().pop() else {
                    break;
                };
                let result = work(item);
                results.lock().unwrap().push((index, result));
            });
        }
    });
    let mut results = results.into_inner().unwrap();
    results.sort_by_key(|(index, _)| *index);
    results.into_iter().map(|(_, result)| result).collect()
}

fn search(output: &Path) {
    let started = Instant::now();
    let screened = parallel_map((0..CANDIDATES).collect(), |seed| {
        let params = ParticleParams::from_seed(seed);
        let (observation, _) = observe(&params, 0, SCREEN_STEPS);
        (seed, observation)
    });
    let cohesive = screened
        .iter()
        .filter(|(_, o)| o.min_cohesion >= COHESION)
        .count();
    let moving = screened
        .iter()
        .filter(|(_, o)| o.min_cohesion >= COHESION && o.mean_speed >= MIN_SPEED)
        .count();
    let survivors: Vec<u64> = screened
        .iter()
        .filter(|(_, o)| o.passes())
        .map(|(seed, _)| *seed)
        .collect();
    println!(
        "ふるい({SCREEN_STEPS} ステップ): {CANDIDATES} 組中、まとまり続けた {cohesive}、そのうえ動いた {moving}、場の半分以上を回った {}({:.0} 秒)",
        survivors.len(),
        started.elapsed().as_secs_f32()
    );

    let verified = parallel_map(survivors, |seed| {
        let params = ParticleParams::from_seed(seed);
        let runs: Vec<(Observation, ParticleWorld)> = (0..VERIFY_SEEDS)
            .map(|trial| observe(&params, trial + 1, VERIFY_STEPS))
            .collect();
        (seed, runs)
    });
    println!(
        "\n確かめ({VERIFY_STEPS} ステップ・初期配置 {VERIFY_SEEDS} 通り)。合格した回数の多い順:\n"
    );
    println!("| 候補 | 合格 | まとまりの最小 | 速さ | 回った割合 | 広がり | 壁際 | 種類×数 | r_max | 力 | 摩擦 |");
    println!("|---|---|---|---|---|---|---|---|---|---|---|");
    let mut ranked: Vec<&(u64, Vec<(Observation, ParticleWorld)>)> = verified.iter().collect();
    ranked.sort_by(|a, b| {
        let passed = |runs: &Vec<(Observation, ParticleWorld)>| {
            runs.iter().filter(|(o, _)| o.passes()).count()
        };
        let coverage = |runs: &Vec<(Observation, ParticleWorld)>| {
            runs.iter().map(|(o, _)| o.coverage).sum::<f32>()
        };
        passed(&b.1)
            .cmp(&passed(&a.1))
            .then(coverage(&b.1).total_cmp(&coverage(&a.1)))
    });
    let mean_of = |runs: &[(Observation, ParticleWorld)], field: fn(&Observation) -> f32| {
        runs.iter().map(|(o, _)| field(o)).sum::<f32>() / runs.len() as f32
    };
    for (seed, runs) in &ranked {
        let passed = runs.iter().filter(|(o, _)| o.passes()).count();
        let params = ParticleParams::from_seed(*seed);
        println!(
            "| {seed} | {passed}/{VERIFY_SEEDS} | {:.2} | {:.3} | {:.2} | {:.1} | {:.2} | {}×{} | {:.1} | {:.3} | {:.2} |",
            runs.iter().map(|(o, _)| o.min_cohesion).fold(1.0, f32::min),
            mean_of(runs, |o| o.mean_speed),
            mean_of(runs, |o| o.coverage),
            mean_of(runs, |o| o.mean_radius),
            mean_of(runs, |o| o.near_wall),
            params.types,
            params.per_type,
            params.r_max,
            params.force,
            params.friction,
        );
        if passed > 0 {
            for (trial, (observation, world)) in runs.iter().enumerate().take(2) {
                write_snapshot(
                    &output.join(format!("candidate_{seed}_{trial}.ppm")),
                    world,
                    &observation.trail,
                );
            }
        }
    }
    println!("\n所要時間 {:.0} 秒", started.elapsed().as_secs_f32());
}

/// 試験の出発点の数と、出発点を作る助走。
const SUITE_STARTS: u64 = 8;
const SUITE_WARMUP: u32 = 1500;
/// 姿を測る窓。
const SIGNATURE_STEPS: u32 = 450;
/// 突いた・散らした後に待つ長さ。
const RECOVERY_STEPS: u32 = 900;

/// 外から見える姿の特徴。
#[derive(Clone, Debug, Default)]
struct Signature {
    min_cohesion: f32,
    speed: f32,
    radius: f32,
    /// 種類ごとの、重心からの平均距離(核と殻のような構造)。
    type_radii: Vec<f32>,
}

fn signature(
    world: &mut ParticleWorld,
    steps: u32,
    mut before_step: impl FnMut(&mut ParticleWorld, u32),
) -> Signature {
    let types = world.params.types;
    let mut result = Signature {
        min_cohesion: 1.0,
        type_radii: vec![0.0; types],
        ..Default::default()
    };
    let (mut samples, mut moves) = (0, 0);
    let mut previous: Option<(f32, f32)> = None;
    for step in 1..=steps {
        before_step(world, step);
        world.step();
        if step % SAMPLE_EVERY != 0 {
            continue;
        }
        let members = world.largest_cluster();
        result.min_cohesion = result
            .min_cohesion
            .min(members.len() as f32 / world.x.len() as f32);
        let ((cx, cy), radius) = centroid_and_radius(world, &members);
        if let Some((px, py)) = previous {
            result.speed += ((cx - px).powi(2) + (cy - py).powi(2)).sqrt() / SAMPLE_EVERY as f32;
            moves += 1;
        }
        previous = Some((cx, cy));
        result.radius += radius;
        for kind in 0..types {
            let of_kind: Vec<f32> = members
                .iter()
                .filter(|&&i| world.kind[i] == kind)
                .map(|&i| ((world.x[i] - cx).powi(2) + (world.y[i] - cy).powi(2)).sqrt())
                .collect();
            result.type_radii[kind] += of_kind.iter().sum::<f32>() / of_kind.len().max(1) as f32;
        }
        samples += 1;
    }
    result.speed /= moves.max(1) as f32;
    result.radius /= samples.max(1) as f32;
    result
        .type_radii
        .iter_mut()
        .for_each(|r| *r /= samples.max(1) as f32);
    result
}

fn relative_difference(a: f32, b: f32) -> f32 {
    let scale = a.abs().max(b.abs());
    if scale <= 1e-6 {
        0.0
    } else {
        (a - b).abs() / scale
    }
}

/// 形の差(広がりと種類ごとの距離の相対差の平均)。
fn shape_difference(a: &Signature, b: &Signature) -> f32 {
    let mut total = relative_difference(a.radius, b.radius);
    for (x, y) in a.type_radii.iter().zip(&b.type_radii) {
        total += relative_difference(*x, *y);
    }
    total / (1 + a.type_radii.len()) as f32
}

fn started_world(seed: u64, start: u64) -> ParticleWorld {
    let mut world = ParticleWorld::new(ParticleParams::from_seed(seed), FIELD, 100 + start);
    for _ in 0..SUITE_WARMUP {
        world.step();
    }
    world
}

fn cluster_center(world: &ParticleWorld) -> (f32, f32) {
    centroid_and_radius(world, &world.largest_cluster()).0
}

fn mean(values: &[f32]) -> f32 {
    values.iter().sum::<f32>() / values.len().max(1) as f32
}

fn suite(seeds: &[u64]) {
    let started = Instant::now();
    for &seed in seeds {
        let params = ParticleParams::from_seed(seed);
        println!(
            "## 候補 {seed}(種類 {}×{}、r_max {:.1}、力 {:.3}、摩擦 {:.2})",
            params.types, params.per_type, params.r_max, params.force, params.friction
        );
        let baselines: Vec<Signature> = parallel_map((0..SUITE_STARTS).collect(), |start| {
            let mut world = started_world(seed, start);
            signature(&mut world, SIGNATURE_STEPS, |_, _| {})
        });
        println!(
            "\n元の姿: 速さ {:.3}、広がり {:.2}、種類ごとの距離 {:?}",
            mean(&baselines.iter().map(|b| b.speed).collect::<Vec<_>>()),
            mean(&baselines.iter().map(|b| b.radius).collect::<Vec<_>>()),
            (0..params.types)
                .map(|k| format!(
                    "{:.2}",
                    mean(
                        &baselines
                            .iter()
                            .map(|b| b.type_radii[k])
                            .collect::<Vec<_>>()
                    )
                ))
                .collect::<Vec<_>>()
        );

        // (2) 突く・散らす
        println!("\n### 突く・散らす(出発点 {SUITE_STARTS} 通り、{RECOVERY_STEPS} ステップ待ってから姿を測る)\n");
        println!(
            "| きっかけ | 戻った(まとまり 0.9 以上) | 形の差(元と) | 待つあいだのまとまりの最小 |"
        );
        println!("|---|---|---|---|");
        #[derive(Clone, Copy)]
        enum Disturbance {
            ScatterLong(f32),
            Poke(f32),
            PokeSpree(f32),
            Scatter(f32),
        }
        let disturbances = [
            ("重心の近くを1回弾く(0.5)", Disturbance::Poke(0.5)),
            ("重心の近くを1回弾く(1.0)", Disturbance::Poke(1.0)),
            ("2秒ごとに30回弾く(1.0)", Disturbance::PokeSpree(1.0)),
            ("粒子の3割を場のどこかへ飛ばす", Disturbance::Scatter(0.3)),
            ("粒子の7割を場のどこかへ飛ばす", Disturbance::Scatter(0.7)),
            (
                "粒子の7割を飛ばし、3000 ステップ待つ",
                Disturbance::ScatterLong(0.7),
            ),
        ];
        for (label, disturbance) in disturbances {
            let outcomes = parallel_map((0..SUITE_STARTS).collect(), |start| {
                let mut world = started_world(seed, start);
                let mut rng = ParticleRng::new(seed * 131 + start);
                let mut lowest = 1.0f32;
                let offset = |rng: &mut ParticleRng| {
                    let angle = rng.range(0.0, std::f32::consts::TAU);
                    (2.0 * angle.cos(), 2.0 * angle.sin())
                };
                match disturbance {
                    Disturbance::Poke(impulse) => {
                        let (cx, cy) = cluster_center(&world);
                        let (ox, oy) = offset(&mut rng);
                        world.poke(cx + ox, cy + oy, 4.5, impulse);
                    }
                    Disturbance::PokeSpree(impulse) => {
                        for _ in 0..30 {
                            let (cx, cy) = cluster_center(&world);
                            let (ox, oy) = offset(&mut rng);
                            world.poke(cx + ox, cy + oy, 4.5, impulse);
                            for _ in 0..30 {
                                world.step();
                            }
                            lowest = lowest
                                .min(world.largest_cluster().len() as f32 / world.x.len() as f32);
                        }
                    }
                    Disturbance::Scatter(share) | Disturbance::ScatterLong(share) => {
                        for i in 0..world.x.len() {
                            if rng.unit() < share {
                                world.x[i] = rng.range(0.0, FIELD - 1e-3);
                                world.y[i] = rng.range(0.0, FIELD - 1e-3);
                            }
                        }
                    }
                }
                let waiting = if matches!(disturbance, Disturbance::ScatterLong(_)) {
                    3000
                } else {
                    RECOVERY_STEPS
                };
                for step in 0..waiting {
                    world.step();
                    if step % SAMPLE_EVERY == 0 {
                        lowest =
                            lowest.min(world.largest_cluster().len() as f32 / world.x.len() as f32);
                    }
                }
                let after = signature(&mut world, SIGNATURE_STEPS, |_, _| {});
                (after, lowest)
            });
            let returned = outcomes
                .iter()
                .filter(|(after, _)| after.min_cohesion >= COHESION)
                .count();
            let differences: Vec<f32> = outcomes
                .iter()
                .zip(&baselines)
                .map(|((after, _), base)| shape_difference(after, base))
                .collect();
            println!(
                "| {label} | {returned}/{SUITE_STARTS} | {:.2}(最大 {:.2}) | {:.2} |",
                mean(&differences),
                differences.iter().copied().fold(0.0, f32::max),
                outcomes
                    .iter()
                    .map(|(_, lowest)| *lowest)
                    .fold(1.0, f32::min)
            );
        }

        // (3) 弱らせて戻す: 元の姿 → 2250 ステップかけて弱らせる → 3000 保つ → 750 で戻す → 3000 保つ
        println!("\n### 弱らせて戻す(ペットの放置と同じ流れ。姿は保つ区間の最後の {SIGNATURE_STEPS} ステップで測る)\n");
        println!("| 弱らせ方 | 崩れなかった | 弱った速さ/元 | 弱った形の差 | 戻った速さ/元 | 戻った形の差 |");
        println!("|---|---|---|---|---|---|");
        #[derive(Clone, Copy)]
        enum Weakening {
            Vigour(f32),
            Force(f32),
            Friction(f32),
        }
        let weakenings = [
            ("進む力 ×0.9", Weakening::Vigour(0.9)),
            ("進む力 ×0.8", Weakening::Vigour(0.8)),
            ("進む力 ×0.5", Weakening::Vigour(0.5)),
            ("進む力 ×0", Weakening::Vigour(0.0)),
            ("力全体 ×0.6", Weakening::Force(0.6)),
            ("摩擦を強める(速度の残り ×0.8)", Weakening::Friction(0.8)),
        ];
        for (label, weakening) in weakenings {
            let outcomes = parallel_map((0..SUITE_STARTS).collect(), |start| {
                let mut world = started_world(seed, start);
                let base_friction = world.params.friction;
                let apply = |world: &mut ParticleWorld, level: f32| match weakening {
                    Weakening::Vigour(target) => world.set_vigour(1.0 + (target - 1.0) * level),
                    Weakening::Force(target) => world.force_scale = 1.0 + (target - 1.0) * level,
                    Weakening::Friction(target) => {
                        world.params.friction = base_friction * (1.0 + (target - 1.0) * level)
                    }
                };
                let mut lowest = 1.0f32;
                let run = |world: &mut ParticleWorld,
                           steps: u32,
                           level: &dyn Fn(u32) -> f32,
                           lowest: &mut f32| {
                    for step in 0..steps {
                        apply(world, level(step));
                        world.step();
                        if step % SAMPLE_EVERY == 0 {
                            *lowest = lowest
                                .min(world.largest_cluster().len() as f32 / world.x.len() as f32);
                        }
                    }
                };
                run(&mut world, 2250, &|step| step as f32 / 2250.0, &mut lowest);
                run(&mut world, 3000 - SIGNATURE_STEPS, &|_| 1.0, &mut lowest);
                let weak = signature(&mut world, SIGNATURE_STEPS, |w, _| apply(w, 1.0));
                run(
                    &mut world,
                    750,
                    &|step| 1.0 - step as f32 / 750.0,
                    &mut lowest,
                );
                run(&mut world, 3000 - SIGNATURE_STEPS, &|_| 0.0, &mut lowest);
                let recovered = signature(&mut world, SIGNATURE_STEPS, |w, _| apply(w, 0.0));
                lowest = lowest.min(weak.min_cohesion).min(recovered.min_cohesion);
                (weak, recovered, lowest)
            });
            let held = outcomes
                .iter()
                .filter(|(_, _, lowest)| *lowest >= COHESION)
                .count();
            let base_speed = mean(&baselines.iter().map(|b| b.speed).collect::<Vec<_>>());
            // 速さの比は、平均どうしの比にする(元の速さがほぼ 0 の出発点で比が跳ねないように)
            let ratio = |pick: fn(&(Signature, Signature, f32)) -> &Signature| {
                mean(&outcomes.iter().map(|o| pick(o).speed).collect::<Vec<_>>())
                    / base_speed.max(1e-6)
            };
            let shape = |pick: fn(&(Signature, Signature, f32)) -> &Signature| {
                mean(
                    &outcomes
                        .iter()
                        .zip(&baselines)
                        .map(|(o, b)| shape_difference(pick(o), b))
                        .collect::<Vec<_>>(),
                )
            };
            println!(
                "| {label} | {held}/{SUITE_STARTS} | {:.2} | {:.2} | {:.2} | {:.2} |",
                ratio(|o| &o.0),
                shape(|o| &o.0),
                ratio(|o| &o.1),
                shape(|o| &o.1)
            );
        }

        // (4) 誘い: 場の中心から 10 セル離れた8点のどれかに誘いを置き、重心がどれだけ近づくか
        println!(
            "\n### 誘いで導く(誘いの点は中心から 10 セルの8方位、{RECOVERY_STEPS} ステップ)\n"
        );
        println!("| 誘いの強さ | 誘いの点までの平均距離(誘いなし → あり) | 5 セル以内にいた割合(なし → あり) | まとまりの最小 |");
        println!("|---|---|---|---|");
        for strength in [0.002f32, 0.005, 0.02] {
            let outcomes = parallel_map(
                (0..SUITE_STARTS)
                    .flat_map(|s| (0..8u32).map(move |d| (s, d)))
                    .collect(),
                |(start, direction)| {
                    let angle = std::f32::consts::TAU * direction as f32 / 8.0;
                    let target = (
                        FIELD / 2.0 + 10.0 * angle.cos(),
                        FIELD / 2.0 + 10.0 * angle.sin(),
                    );
                    let measure = |lure: Option<(f32, f32, f32)>| {
                        let mut world = started_world(seed, start);
                        world.lure = lure;
                        let (mut distance, mut near, mut samples, mut lowest) = (0.0, 0, 0, 1.0f32);
                        for step in 1..=RECOVERY_STEPS {
                            world.step();
                            if step % SAMPLE_EVERY == 0 {
                                let members = world.largest_cluster();
                                lowest = lowest.min(members.len() as f32 / world.x.len() as f32);
                                let ((cx, cy), _) = centroid_and_radius(&world, &members);
                                let d = ((cx - target.0).powi(2) + (cy - target.1).powi(2)).sqrt();
                                distance += d;
                                if d < 5.0 {
                                    near += 1;
                                }
                                samples += 1;
                            }
                        }
                        (
                            distance / samples as f32,
                            near as f32 / samples as f32,
                            lowest,
                        )
                    };
                    (measure(None), measure(Some((target.0, target.1, strength))))
                },
            );
            println!(
                "| {strength} | {:.1} → {:.1} | {:.2} → {:.2} | {:.2} |",
                mean(&outcomes.iter().map(|(a, _)| a.0).collect::<Vec<_>>()),
                mean(&outcomes.iter().map(|(_, b)| b.0).collect::<Vec<_>>()),
                mean(&outcomes.iter().map(|(a, _)| a.1).collect::<Vec<_>>()),
                mean(&outcomes.iter().map(|(_, b)| b.1).collect::<Vec<_>>()),
                outcomes.iter().map(|(_, b)| b.2).fold(1.0, f32::min)
            );
        }

        // (5) 重さ: 1スレッドで 20000 ステップ
        let mut world = started_world(seed, 0);
        let timer = Instant::now();
        for _ in 0..20_000 {
            world.step();
        }
        let per_step = timer.elapsed().as_secs_f64() / 20_000.0;
        println!(
            "\n1ステップ {:.1} µs(粒子 {} 個、PC 1スレッド)\n",
            per_step * 1e6,
            params.count()
        );
    }

    // 比べる基準: いまの Lenia(O2u、M5Stack と同じ 43×32)の1ステップ
    let animal = vmc_pet_body::load_animal("O2u").unwrap();
    let mut lenia = vmc_pet_body::Lenia::new(animal.params.clone());
    let mut field = vmc_pet_body::Field::new(43, 32);
    field.place_centered(&animal.pattern);
    for _ in 0..300 {
        lenia.step(&mut field, 1.0);
    }
    let timer = Instant::now();
    for _ in 0..5_000 {
        lenia.step(&mut field, 1.0);
    }
    println!(
        "基準: いまの Lenia(O2u、43×32)の1ステップ {:.1} µs(M5Stack の実測は約 62 ms)",
        timer.elapsed().as_secs_f64() / 5_000.0 * 1e6
    );
    println!("所要時間 {:.0} 秒", started.elapsed().as_secs_f32());
}

/// 誘いの強さを上げていったとき、導ける度合いと、誘っているあいだ・やめた後の姿を測る。
///
/// 誘いの点は場の中心から 10 セルの8方位。900 ステップ誘い、そのあいだの居場所と姿を測る。続けて誘いを
/// やめ、`AFTER_LURE_STEPS` 待ってから姿を測る。姿の差は、誘いなしで同じ出発点から測った元の姿と比べる。
/// 誘いが無くても時間が経つと止まることがあるので、誘いなしで同じ長さだけ動かしたときに止まった回数も数える。
fn lure_strengths(seeds: &[u64]) {
    const STRENGTHS: [f32; 6] = [0.02, 0.05, 0.1, 0.15, 0.2, 0.3];
    const AFTER_LURE_STEPS: u32 = 3000;
    /// これより遅ければ止まったとみなす(探索の合格の速さ 0.05 の半分未満)。
    const STOPPED_SPEED: f32 = 0.02;
    let started = Instant::now();
    for &seed in seeds {
        let params = ParticleParams::from_seed(seed);
        println!(
            "## 候補 {seed}(種類 {}×{}、力 {:.3})\n",
            params.types, params.per_type, params.force
        );
        let baselines: Vec<Signature> = parallel_map((0..SUITE_STARTS).collect(), |start| {
            let mut world = started_world(seed, start);
            signature(&mut world, SIGNATURE_STEPS, |_, _| {})
        });
        let jobs: Vec<(u64, u32)> = (0..SUITE_STARTS)
            .flat_map(|s| (0..8u32).map(move |d| (s, d)))
            .collect();
        let no_lure = parallel_map(jobs.clone(), |(start, direction)| {
            let target = lure_target(direction);
            let mut world = started_world(seed, start);
            distance_to(&mut world, target, RECOVERY_STEPS).0
        });
        // 誘いなしで、誘う試行と同じ長さだけ動かしたとき(出発点ごとに1回)
        let untouched = parallel_map((0..SUITE_STARTS).collect(), |start| {
            let mut world = started_world(seed, start);
            for _ in 0..RECOVERY_STEPS + AFTER_LURE_STEPS {
                world.step();
            }
            signature(&mut world, SIGNATURE_STEPS, |_, _| {})
        });
        println!(
            "誘いなしで同じ長さだけ動かしたとき、止まっていたのは {}/{SUITE_STARTS}\n",
            untouched.iter().filter(|s| s.speed < STOPPED_SPEED).count()
        );
        println!("| 誘いの強さ | 誘いの点までの平均距離(なし → あり) | 5 セル以内にいた割合(なし → あり) | 誘っているあいだのまとまりの最小 | 誘っているあいだの形の差 | やめた後に戻った(まとまり 0.9 以上) | やめた後の形の差 | やめた後の速さ/元 | やめた後に止まっていた |");
        println!("|---|---|---|---|---|---|---|---|---|");
        let base_speed = mean(&baselines.iter().map(|b| b.speed).collect::<Vec<_>>());
        for strength in STRENGTHS {
            let outcomes = parallel_map(jobs.clone(), |(start, direction)| {
                let target = lure_target(direction);
                let mut world = started_world(seed, start);
                world.lure = Some((target.0, target.1, strength));
                let (near, lowest) =
                    distance_to(&mut world, target, RECOVERY_STEPS - SIGNATURE_STEPS);
                let during = signature(&mut world, SIGNATURE_STEPS, |_, _| {});
                world.lure = None;
                for _ in 0..AFTER_LURE_STEPS {
                    world.step();
                }
                let after = signature(&mut world, SIGNATURE_STEPS, |_, _| {});
                (near, lowest.min(during.min_cohesion), during, after, start)
            });
            let pick =
                |f: &dyn Fn(&LureOutcome) -> f32| mean(&outcomes.iter().map(f).collect::<Vec<_>>());
            let returned = outcomes
                .iter()
                .filter(|o| o.3.min_cohesion >= COHESION)
                .count();
            let stopped = outcomes
                .iter()
                .filter(|o| o.3.speed < STOPPED_SPEED)
                .count();
            println!(
                "| {strength} | {:.1} → {:.1} | {:.2} → {:.2} | {:.2} | {:.2} | {returned}/{} | {:.2} | {:.2} | {stopped}/{} |",
                mean(&no_lure.iter().map(|n| n.distance).collect::<Vec<_>>()),
                pick(&|o| o.0.distance),
                mean(&no_lure.iter().map(|n| n.near).collect::<Vec<_>>()),
                pick(&|o| o.0.near),
                outcomes.iter().map(|o| o.1).fold(1.0, f32::min),
                pick(&|o| shape_difference(&o.2, &baselines[o.4 as usize])),
                outcomes.len(),
                pick(&|o| shape_difference(&o.3, &baselines[o.4 as usize])),
                pick(&|o| o.3.speed) / base_speed.max(1e-6),
                outcomes.len(),
            );
        }
        println!();
    }
    println!("所要時間 {:.0} 秒", started.elapsed().as_secs_f32());
}

fn lure_target(direction: u32) -> (f32, f32) {
    let angle = std::f32::consts::TAU * direction as f32 / 8.0;
    (
        FIELD / 2.0 + 10.0 * angle.cos(),
        FIELD / 2.0 + 10.0 * angle.sin(),
    )
}

/// 誘いの試行1回の結果: (誘っているあいだの居場所, まとまりの最小, 誘っているあいだの姿, やめた後の姿, 出発点)。
type LureOutcome = (Nearness, f32, Signature, Signature, u64);

/// 誘いの点までの距離の平均と、5 セル以内にいた割合。
#[derive(Clone, Copy, Default)]
struct Nearness {
    distance: f32,
    near: f32,
}

/// `steps` だけ進めながら、最大の塊の重心と点 `target` の距離を 1 秒ごとに測る。まとまりの最小も返す。
fn distance_to(world: &mut ParticleWorld, target: (f32, f32), steps: u32) -> (Nearness, f32) {
    let (mut distance, mut near, mut samples, mut lowest) = (0.0, 0, 0, 1.0f32);
    for step in 1..=steps {
        world.step();
        if step % SAMPLE_EVERY == 0 {
            let members = world.largest_cluster();
            lowest = lowest.min(members.len() as f32 / world.x.len() as f32);
            let ((cx, cy), _) = centroid_and_radius(world, &members);
            let d = ((cx - target.0).powi(2) + (cy - target.1).powi(2)).sqrt();
            distance += d;
            if d < 5.0 {
                near += 1;
            }
            samples += 1;
        }
    }
    let samples = samples.max(1) as f32;
    (
        Nearness {
            distance: distance / samples,
            near: near as f32 / samples,
        },
        lowest,
    )
}

/// 崩れた体を、手で書いた簡単な行動でまとめ直せるか(学習ループの前に、行動で報酬が変わるかを測る)。
///
/// 崩し方ごとに `REGROUP_TRIALS` 回、出発点と崩し方の乱数をそろえて、行動なしと各行動を対にして比べる。
/// 行動は `REGROUP_WINDOW` ステップのあいだだけ効かせ、そのあと止めて姿を測る(行動をやめても保たれるか)。
fn regroup(seeds: &[u64]) {
    const REGROUP_TRIALS: u64 = 16;
    const REGROUP_WINDOW: u32 = 1800;

    #[derive(Clone, Copy)]
    enum Breakage {
        Scatter(f32),
        PokeSpree,
    }
    #[derive(Clone, Copy)]
    enum Action {
        Nothing,
        /// 最大の塊の重心へ誘う(強さ, 届く距離)。重心は 1 秒ごとに測り直す。
        LureToCluster(f32, f32),
        /// 自分で進む力を `vigour` 倍にして `steps` ステップ待ち、元に戻す。
        Calm(f32, u32),
    }
    impl Action {
        fn label(self) -> String {
            match self {
                Self::Nothing => "何もしない".to_string(),
                Self::LureToCluster(strength, radius) => {
                    format!("塊の重心へ誘う(強さ {strength}、距離 {radius})")
                }
                Self::Calm(vigour, steps) => {
                    format!("進む力を ×{vigour} にして {steps} ステップ待つ")
                }
            }
        }
    }
    let breakages = [
        ("粒子の3割を散らす", Breakage::Scatter(0.3)),
        ("粒子の7割を散らす", Breakage::Scatter(0.7)),
        ("2秒ごとに30回弾く(1.0)", Breakage::PokeSpree),
    ];
    let actions = [
        Action::Nothing,
        Action::LureToCluster(0.005, 10.0),
        Action::LureToCluster(0.02, 10.0),
        Action::LureToCluster(0.005, 45.0),
        Action::LureToCluster(0.02, 45.0),
        Action::LureToCluster(0.05, 45.0),
        Action::Calm(0.3, 300),
        Action::Calm(0.3, 900),
    ];

    let started = Instant::now();
    for &seed in seeds {
        let params = ParticleParams::from_seed(seed);
        println!(
            "## 候補 {seed}(種類 {}×{})\n",
            params.types, params.per_type
        );
        let baselines: Vec<Signature> = parallel_map((0..REGROUP_TRIALS).collect(), |trial| {
            let mut world = started_world(seed, trial);
            signature(&mut world, SIGNATURE_STEPS, |_, _| {})
        });
        let base_speed = mean(&baselines.iter().map(|b| b.speed).collect::<Vec<_>>());
        for (label, breakage) in breakages {
            println!(
                "### {label}(対にした試行 {REGROUP_TRIALS} 回、行動は {REGROUP_WINDOW} ステップ)\n"
            );
            println!("| 行動 | まとまり 0.9 に戻るまで(秒。戻らなければ窓の長さ) | 何もしないより早かった/遅かった | 窓の終わりに割れたまま | 行動をやめた後に割れていた | やめた後の形の差 | やめた後の速さ/元 |");
            println!("|---|---|---|---|---|---|---|");
            let jobs: Vec<(usize, u64)> = (0..actions.len())
                .flat_map(|a| (0..REGROUP_TRIALS).map(move |t| (a, t)))
                .collect();
            // (行動, 試行, 戻るまでのステップ, 窓の終わりに割れていた, やめた後の姿)
            let results = parallel_map(jobs, |(a, trial)| {
                let mut world = started_world(seed, trial);
                let mut rng = ParticleRng::new(seed * 7919 + trial);
                match breakage {
                    Breakage::Scatter(share) => {
                        for i in 0..world.x.len() {
                            if rng.unit() < share {
                                world.x[i] = rng.range(0.0, FIELD - 1e-3);
                                world.y[i] = rng.range(0.0, FIELD - 1e-3);
                            }
                        }
                    }
                    Breakage::PokeSpree => {
                        for _ in 0..30 {
                            let (cx, cy) = cluster_center(&world);
                            let angle = rng.range(0.0, std::f32::consts::TAU);
                            world.poke(cx + 2.0 * angle.cos(), cy + 2.0 * angle.sin(), 4.5, 1.0);
                            for _ in 0..30 {
                                world.step();
                            }
                        }
                    }
                }
                let mut recovered_at = REGROUP_WINDOW;
                let mut broken_at_end = false;
                for step in 1..=REGROUP_WINDOW {
                    match actions[a] {
                        Action::Nothing => {}
                        Action::LureToCluster(strength, radius) => {
                            if step % SAMPLE_EVERY == 1 {
                                let (cx, cy) = cluster_center(&world);
                                world.lure = Some((cx, cy, strength));
                                world.lure_radius = radius;
                            }
                        }
                        Action::Calm(vigour, steps) => {
                            world.set_vigour(if step <= steps { vigour } else { 1.0 });
                        }
                    }
                    world.step();
                    if step % SAMPLE_EVERY == 0 {
                        let cohesion = world.largest_cluster().len() as f32 / world.x.len() as f32;
                        if cohesion >= COHESION && recovered_at == REGROUP_WINDOW {
                            recovered_at = step;
                        }
                        if step == REGROUP_WINDOW {
                            broken_at_end = cohesion < COHESION;
                        }
                    }
                }
                world.lure = None;
                world.set_vigour(1.0);
                let after = signature(&mut world, SIGNATURE_STEPS, |_, _| {});
                (a, trial, recovered_at, broken_at_end, after)
            });
            let of = |a: usize| {
                let mut rows: Vec<_> = results.iter().filter(|r| r.0 == a).collect();
                rows.sort_by_key(|r| r.1);
                rows
            };
            let nothing = of(0);
            for (a, action) in actions.iter().enumerate() {
                let rows = of(a);
                let seconds: Vec<f32> = rows.iter().map(|r| r.2 as f32 / 15.0).collect();
                let faster = rows.iter().zip(&nothing).filter(|(r, n)| r.2 < n.2).count();
                let slower = rows.iter().zip(&nothing).filter(|(r, n)| r.2 > n.2).count();
                let broken_end = rows.iter().filter(|r| r.3).count();
                let broken_after = rows.iter().filter(|r| r.4.min_cohesion < COHESION).count();
                let shape: Vec<f32> = rows
                    .iter()
                    .map(|r| shape_difference(&r.4, &baselines[r.1 as usize]))
                    .collect();
                let speed = mean(&rows.iter().map(|r| r.4.speed).collect::<Vec<_>>());
                println!(
                    "| {} | {:.1} ± {:.1} | {faster}/{slower} | {broken_end}/{REGROUP_TRIALS} | {broken_after}/{REGROUP_TRIALS} | {:.2} | {:.2} |",
                    action.label(),
                    mean(&seconds),
                    standard_deviation(&seconds),
                    mean(&shape),
                    speed / base_speed.max(1e-6),
                );
            }
            println!();
        }
    }
    println!("所要時間 {:.0} 秒", started.elapsed().as_secs_f32());
}

fn standard_deviation(values: &[f32]) -> f32 {
    let m = mean(values);
    (values.iter().map(|v| (v - m) * (v - m)).sum::<f32>() / values.len().max(1) as f32).sqrt()
}

/// 誘いの代償を入れた報酬の地形。行動のパラメータ(強さ・届く距離・誘いはじめるまとまりのしきい値)を格子で振り、
/// 報酬の中身を測って、代償の重みを変えたときに報酬が最大になる組が範囲の内側に来るかを見る。
///
/// 場面は、崩していない体・3割を散らした体・7割を散らした体の3つ。どの行動も同じ出発点と散らし方の乱数で走らせる。
/// 行動は 1 秒ごとにまとまりを測り、しきい値を下回っていれば最大の塊の重心へ誘い、そうでなければ誘わない。
///
/// 代償は「吸い寄せの激しさ」: 粒子の速さの平均が窓の中でどこまで上がったか(ピーク)を、何もしない場合と対にした
/// 増分で測る。加えて、崩れていないのに誘っていた時間も代償にする。しきい値つきで強く引くと、1秒ほどで戻って誘いが止まるので、誘いの量やまとまっているときの形の差では
/// 代償が小さく出てしまう。見ている人にとっての代償は、散らばった粒子が一気に吸い寄せられる不自然な速さだと考えた。
fn landscape(seeds: &[u64]) {
    const TRIALS: u64 = 16;
    const WINDOW: u32 = 1800;
    const STRENGTHS: [f32; 7] = [0.0, 0.005, 0.01, 0.02, 0.05, 0.1, 0.2];
    const RADII: [f32; 3] = [10.0, 20.0, 45.0];
    /// 1.01 は「いつも誘う」。
    const THRESHOLDS: [f32; 5] = [0.6, 0.8, 0.9, 0.95, 1.01];
    const SCATTERS: [f32; 3] = [0.0, 0.3, 0.7];
    /// 吸い寄せの激しさの重み。
    /// 報酬 = −(3割・7割の戻るまでの秒の平均 / 120) − 重み × (粒子の速さのピークの増分 / 崩していない体の粒子の速さ)。
    const SNAP_WEIGHTS: [f32; 8] = [0.0, 0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 2.0];
    /// 崩れていないのに誘う代償の重み(崩していない体で誘っていた時間の割合に掛ける)。いつも引き続けると、
    /// ユーザーが散らしても崩れて見えなくなる(放置で弱った様子を打ち消すのと同じ衝突)ため。
    const IDLE_WEIGHTS: [f32; 2] = [0.0, 1.0];

    /// 1回の走らせの結果。
    #[derive(Clone, Copy, Default)]
    struct Run {
        /// まとまり 0.9 に戻るまでの秒(崩していない体では 0。戻らなければ窓の長さ)。
        seconds: f32,
        /// 窓の終わりに割れていた。
        broken: bool,
        /// まとまり 0.9 以上のときの形の差(元の姿と)の平均。
        distortion: f32,
        /// 1ステップあたりの誘いの強さの平均。
        effort: f32,
        /// 誘っていた時間の割合。
        active: f32,
        /// 粒子の速さの平均の、窓の中でのピーク。
        peak_speed: f32,
        /// 粒子の速さの平均の、窓全体での平均。
        mean_speed: f32,
    }

    struct Summary {
        config: (f32, f32, f32),
        /// 3割・7割を散らしたときの戻るまでの秒。
        seconds: [f32; 2],
        broken: usize,
        /// まとまっているときの形の差の増分(何もしないと対にした平均、3場面)。
        distortion: f32,
        effort: f32,
        active_intact: f32,
        /// 散らした2場面での、粒子の速さのピークの増分(何もしないと対にし、下がった分は 0 とした平均)。
        snap: f32,
    }

    let started = Instant::now();
    for &seed in seeds {
        println!("## 候補 {seed}\n");
        let baselines: Vec<Signature> = parallel_map((0..TRIALS).collect(), |trial| {
            let mut world = started_world(seed, trial);
            signature(&mut world, SIGNATURE_STEPS, |_, _| {})
        });
        let mut configs = vec![(0.0, RADII[0], THRESHOLDS[0])];
        for &strength in &STRENGTHS[1..] {
            for &radius in &RADII {
                for &threshold in &THRESHOLDS {
                    configs.push((strength, radius, threshold));
                }
            }
        }
        let jobs: Vec<(usize, usize, u64)> = (0..configs.len())
            .flat_map(|c| {
                (0..SCATTERS.len()).flat_map(move |s| (0..TRIALS).map(move |t| (c, s, t)))
            })
            .collect();
        let runs = parallel_map(jobs, |(c, s, trial)| {
            let (strength, radius, threshold) = configs[c];
            let base = &baselines[trial as usize];
            let mut world = started_world(seed, trial);
            let mut rng = ParticleRng::new(seed * 7919 + trial);
            for i in 0..world.x.len() {
                if rng.unit() < SCATTERS[s] {
                    world.x[i] = rng.range(0.0, FIELD - 1e-3);
                    world.y[i] = rng.range(0.0, FIELD - 1e-3);
                }
            }
            world.lure_radius = radius;
            let mut run = Run {
                seconds: WINDOW as f32 / 15.0,
                ..Default::default()
            };
            let (mut distortion_total, mut intact_samples) = (0.0, 0);
            let (mut active_steps, mut effort_total) = (0, 0.0);
            let mut recovered = false;
            let mut cohesion = 0.0;
            for step in 1..=WINDOW {
                if step % SAMPLE_EVERY == 1 {
                    let members = world.largest_cluster();
                    let current = members.len() as f32 / world.x.len() as f32;
                    world.lure = if strength > 0.0 && current < threshold {
                        let ((cx, cy), _) = centroid_and_radius(&world, &members);
                        Some((cx, cy, strength))
                    } else {
                        None
                    };
                }
                if let Some((_, _, applied)) = world.lure {
                    active_steps += 1;
                    effort_total += applied;
                }
                world.step();
                let particle_speed = world
                    .vx
                    .iter()
                    .zip(&world.vy)
                    .map(|(vx, vy)| (vx * vx + vy * vy).sqrt())
                    .sum::<f32>()
                    / world.x.len() as f32;
                run.peak_speed = run.peak_speed.max(particle_speed);
                run.mean_speed += particle_speed / WINDOW as f32;
                if step % SAMPLE_EVERY != 0 {
                    continue;
                }
                let members = world.largest_cluster();
                cohesion = members.len() as f32 / world.x.len() as f32;
                if cohesion < COHESION {
                    continue;
                }
                if !recovered {
                    run.seconds = if SCATTERS[s] == 0.0 {
                        0.0
                    } else {
                        step as f32 / 15.0
                    };
                    recovered = true;
                }
                let ((cx, cy), spread) = centroid_and_radius(&world, &members);
                let types = world.params.types;
                let mut now = Signature {
                    radius: spread,
                    type_radii: vec![0.0; types],
                    ..Default::default()
                };
                for kind in 0..types {
                    let of_kind: Vec<f32> = members
                        .iter()
                        .filter(|&&i| world.kind[i] == kind)
                        .map(|&i| ((world.x[i] - cx).powi(2) + (world.y[i] - cy).powi(2)).sqrt())
                        .collect();
                    now.type_radii[kind] =
                        of_kind.iter().sum::<f32>() / of_kind.len().max(1) as f32;
                }
                distortion_total += shape_difference(&now, base);
                intact_samples += 1;
            }
            run.broken = cohesion < COHESION;
            run.distortion = distortion_total / intact_samples.max(1) as f32;
            run.effort = effort_total / WINDOW as f32;
            run.active = active_steps as f32 / WINDOW as f32;
            (c, s, trial, run)
        });

        let find = |c: usize, s: usize| -> Vec<Run> {
            let mut rows: Vec<_> = runs.iter().filter(|r| r.0 == c && r.1 == s).collect();
            rows.sort_by_key(|r| r.2);
            rows.into_iter().map(|r| r.3).collect()
        };
        // 行動なし(強さ 0)を、同じ場面・同じ試行の基準にする
        let nothing: Vec<Vec<Run>> = (0..SCATTERS.len()).map(|s| find(0, s)).collect();
        let intact_particle_speed =
            mean(&nothing[0].iter().map(|r| r.mean_speed).collect::<Vec<_>>());
        let paired = |rows: &[Run], base: &[Run], pick: fn(&Run) -> f32| {
            mean(
                &rows
                    .iter()
                    .zip(base)
                    .map(|(r, n)| pick(r) - pick(n))
                    .collect::<Vec<_>>(),
            )
        };
        // 吸い寄せの激しさは、何もしないより速さのピークが上がった分だけを数える。下がった分を得にすると、
        // 近くの塊だけを強く締めて粒子の動きを鈍らせる行動(まとまり直すのはかえって遅い)が選ばれてしまった
        let snap_increase = |rows: &[Run], base: &[Run]| {
            mean(
                &rows
                    .iter()
                    .zip(base)
                    .map(|(r, n)| (r.peak_speed - n.peak_speed).max(0.0))
                    .collect::<Vec<_>>(),
            )
        };
        let summaries: Vec<Summary> = (0..configs.len())
            .map(|c| {
                let scenes: Vec<Vec<Run>> = (0..SCATTERS.len()).map(|s| find(c, s)).collect();
                let seconds_of =
                    |s: usize| mean(&scenes[s].iter().map(|r| r.seconds).collect::<Vec<_>>());
                Summary {
                    config: configs[c],
                    seconds: [seconds_of(1), seconds_of(2)],
                    broken: scenes.iter().flatten().filter(|r| r.broken).count(),
                    distortion: (0..SCATTERS.len())
                        .map(|s| paired(&scenes[s], &nothing[s], |r| r.distortion))
                        .sum::<f32>()
                        / SCATTERS.len() as f32,
                    effort: mean(
                        &scenes
                            .iter()
                            .flatten()
                            .map(|r| r.effort)
                            .collect::<Vec<_>>(),
                    ),
                    active_intact: mean(&scenes[0].iter().map(|r| r.active).collect::<Vec<_>>()),
                    snap: (snap_increase(&scenes[1], &nothing[1])
                        + snap_increase(&scenes[2], &nothing[2]))
                        / 2.0,
                }
            })
            .collect();

        println!("崩していない体の粒子の速さの平均: {intact_particle_speed:.4} セル/ステップ\n");
        println!("| 強さ | 距離 | しきい値 | 戻るまで(秒): 3割 / 7割 | 割れたまま(3場面×{TRIALS}) | 吸い寄せの激しさ(速さのピークの増分/崩していない体の速さ) | まとまっているときの形の差の増分 | 誘いの量 | 崩していない体で誘っていた割合 |");
        println!("|---|---|---|---|---|---|---|---|---|");
        let threshold_label = |threshold: f32| {
            if threshold > 1.0 {
                "いつも".to_string()
            } else {
                format!("{threshold}")
            }
        };
        for summary in &summaries {
            let (strength, radius, threshold) = summary.config;
            println!(
                "| {strength} | {radius} | {} | {:.1} / {:.1} | {} | {:.2} | {:+.3} | {:.4} | {:.2} |",
                threshold_label(threshold),
                summary.seconds[0],
                summary.seconds[1],
                summary.broken,
                summary.snap / intact_particle_speed,
                summary.distortion,
                summary.effort,
                summary.active_intact,
            );
        }

        for idle_weight in IDLE_WEIGHTS {
            println!(
                "\n### 崩れていないのに誘う代償の重み {idle_weight} で、吸い寄せの激しさの重みを変えたときの最良の組\n"
            );
            println!("報酬 = −(3割・7割の戻るまでの秒の平均 / 120) − 重み × 吸い寄せの激しさ − {idle_weight} × 崩していない体で誘っていた割合\n");
            println!("| 吸い寄せの重み | 最良の強さ | 距離 | しきい値 | 戻るまで(秒) | 吸い寄せの激しさ | 崩していない体で誘っていた割合 | 報酬 | 強さは格子の端か |");
            println!("|---|---|---|---|---|---|---|---|---|");
            for weight in SNAP_WEIGHTS {
                let reward = |s: &Summary| {
                    -((s.seconds[0] + s.seconds[1]) / 2.0) / 120.0
                        - weight * s.snap / intact_particle_speed
                        - idle_weight * s.active_intact
                };
                let best = summaries
                    .iter()
                    .max_by(|a, b| reward(a).total_cmp(&reward(b)))
                    .unwrap();
                let (strength, radius, threshold) = best.config;
                let edge = if strength == STRENGTHS[STRENGTHS.len() - 1] {
                    "上端"
                } else if strength == 0.0 {
                    "何もしない"
                } else if strength == STRENGTHS[1] {
                    "下端"
                } else {
                    "内側"
                };
                println!(
                    "| {weight} | {strength} | {radius} | {} | {:.1} | {:.2} | {:.2} | {:.3} | {edge} |",
                    threshold_label(threshold),
                    (best.seconds[0] + best.seconds[1]) / 2.0,
                    best.snap / intact_particle_speed,
                    best.active_intact,
                    reward(best),
                );
            }
        }
        println!();
    }
    println!("所要時間 {:.0} 秒", started.elapsed().as_secs_f32());
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("search") => {
            let output = PathBuf::from(args.get(1).expect("出力先のディレクトリを渡す"));
            fs::create_dir_all(&output).expect("出力先を作れない");
            search(&output);
        }
        Some("landscape") => {
            let seeds: Vec<u64> = args[1..]
                .iter()
                .map(|s| s.parse().expect("候補の番号"))
                .collect();
            landscape(&seeds);
        }
        Some("regroup") => {
            let seeds: Vec<u64> = args[1..]
                .iter()
                .map(|s| s.parse().expect("候補の番号"))
                .collect();
            regroup(&seeds);
        }
        Some("lure") => {
            let seeds: Vec<u64> = args[1..]
                .iter()
                .map(|s| s.parse().expect("候補の番号"))
                .collect();
            lure_strengths(&seeds);
        }
        Some("suite") => {
            let seeds: Vec<u64> = args[1..]
                .iter()
                .map(|s| s.parse().expect("候補の番号"))
                .collect();
            suite(&seeds);
        }
        _ => eprintln!(
            "使い方: particle_trial search <出力先> | suite <候補の番号>... | lure <候補の番号>... | regroup <候補の番号>... | landscape <候補の番号>..."
        ),
    }
}
