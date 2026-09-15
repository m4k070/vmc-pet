//! 【実験】S1s の規則のもとで、滑る形と止まった形は、どんな初期状態からどれくらいの割合で現れるか。
//!
//! 「別の形から戻る帰り道はあるか」(docs/DESIGN.md)で、止まったドーナツ形の S1s は、どのきっかけ
//! でも、ほかの生物の値を経由しても、滑る姿へ戻らなかった。滑る S1s は animals.json の初期パターン
//! から育てたときにだけ現れる「谷の狭い形」で、止まった形のほうが広い範囲から流れ込む「谷の深い形」
//! なのではないか、という仮説を確かめる(docs/DESIGN.md「滑る S1s は谷の狭い形か」)。
//!
//! S1s のパラメータのまま(いまの Lenia の規則、成長の強さ 1.0、場 32×32)、いろいろな初期状態から
//! 6000 ステップ育て、3000 ステップ目と 6000 ステップ目の手前 900 ステップの速さで分ける。
//!
//! 初期状態は、S1s のパターン(向き・ゆらぎ・傷を変える)、ほかの生物のパターン、乱数の円盤・
//! 楕円・輪。S1s は総量の多い体で、軽い初期状態は形と関係なく崩壊してしまうので、S1s 以外の
//! 初期状態は総量を S1s のパターンにそろえた版も試す。
//!
//! 使い方: `cargo run --release -p vmc-pet-body --example glide_or_stop -- <出力先>`。
//! 出力先に、初期状態ごとに試行0〜3の最後の姿を PGM で書き出す。ペット本体の振る舞いには触れない。

use std::f32::consts::TAU;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread;
use std::time::Instant;

use vmc_pet_body::animal::Pattern;
use vmc_pet_body::{load_animal, Animal, Field, Lenia};

const FIELD_SIZE: usize = 32;
const CELLS: usize = FIELD_SIZE * FIELD_SIZE;
const COLLAPSE_MASS: f32 = 5.0;
/// はっきり見える点とみなす値(`persistence_expression.rs` と同じ)。
const CLEARLY_VISIBLE: f32 = 0.2;
const EXPLODED_AREA: usize = CELLS / 2;

const TOTAL_STEPS: u32 = 6_000;
/// 速さを測る区間の終わり。それぞれ手前の `MEASURE_STEPS` で測る。
const CHECKPOINTS: [u32; 2] = [3_000, 6_000];
const MEASURE_STEPS: u32 = 900;
/// これより速ければ「滑る」。滑る S1s は 0.356 だった。
const GLIDING_SPEED: f32 = 0.1;
/// これより遅ければ「止まる」。止まった形は 0.010 以下だった。
const STOPPED_SPEED: f32 = 0.02;
/// 画像を書き出す試行の数(初期状態ごと)。
const SNAPSHOT_TRIALS: u64 = 4;

/// パターンの向き。
#[derive(Clone, Copy)]
enum Orientation {
    AsIs,
    Rotate90,
    Rotate180,
    MirrorX,
}

/// 初期状態の作り方。
#[derive(Clone, Copy)]
enum Seed {
    /// 生物のパターンを、向きを変え、各セルに ±`noise` の割合のゆらぎを掛けて置く。
    Pattern {
        code: &'static str,
        orientation: Orientation,
        noise: f32,
    },
    /// 半径 `radius` の円盤の中を一様乱数(0〜1)で埋める。
    RandomDisk { radius: f32 },
    /// 長半径 8・短半径 4 のガウスの楕円形の塊を、乱数の向きに置く(ゆらぎ 5%)。
    Ellipse,
    /// 半径 6・太さ 2 の輪(値は 0.6〜1 の乱数)。止まったドーナツ形に近い、対称な初期状態。
    Ring,
    /// S1s のパターンの左半分を消す。
    S1sHalfErased,
    /// S1s のパターンを 3×3 の平均で2回ぼかす。
    S1sBlurred,
    /// 中身の初期状態の値に同じ倍率を掛け(1 で切り詰め)、総量を S1s のパターンにそろえる。
    /// S1s は総量の多い体なので、軽い初期状態は形の良し悪しと関係なく崩壊してしまうため。
    MassMatched(&'static Seed),
}

/// S1s のパターンをそのまま置く初期状態(傷をつける初期状態の元にする)。
const S1S_AS_IS: Seed = Seed::Pattern {
    code: "S1s",
    orientation: Orientation::AsIs,
    noise: 0.001,
};
/// 総量をそろえるとき、倍率を掛けて 1 で切り詰めることを繰り返す回数(切り詰めで減った分を補う)。
const MASS_MATCH_ITERATIONS: usize = 5;

const SEEDS: [(&str, Seed, u64); 25] = [
    (
        "S1s そのまま",
        Seed::Pattern {
            code: "S1s",
            orientation: Orientation::AsIs,
            noise: 0.001,
        },
        8,
    ),
    (
        "S1s 90度回転",
        Seed::Pattern {
            code: "S1s",
            orientation: Orientation::Rotate90,
            noise: 0.001,
        },
        8,
    ),
    (
        "S1s 180度回転",
        Seed::Pattern {
            code: "S1s",
            orientation: Orientation::Rotate180,
            noise: 0.001,
        },
        8,
    ),
    (
        "S1s 左右反転",
        Seed::Pattern {
            code: "S1s",
            orientation: Orientation::MirrorX,
            noise: 0.001,
        },
        8,
    ),
    (
        "S1s ゆらぎ 5%",
        Seed::Pattern {
            code: "S1s",
            orientation: Orientation::AsIs,
            noise: 0.05,
        },
        8,
    ),
    (
        "S1s ゆらぎ 20%",
        Seed::Pattern {
            code: "S1s",
            orientation: Orientation::AsIs,
            noise: 0.2,
        },
        8,
    ),
    (
        "O2u のパターン",
        Seed::Pattern {
            code: "O2u",
            orientation: Orientation::AsIs,
            noise: 0.001,
        },
        8,
    ),
    (
        "OG2g のパターン",
        Seed::Pattern {
            code: "OG2g",
            orientation: Orientation::AsIs,
            noise: 0.001,
        },
        8,
    ),
    (
        "2S1v のパターン",
        Seed::Pattern {
            code: "2S1v",
            orientation: Orientation::AsIs,
            noise: 0.001,
        },
        8,
    ),
    ("乱数の円盤 半径5", Seed::RandomDisk { radius: 5.0 }, 32),
    ("乱数の円盤 半径7", Seed::RandomDisk { radius: 7.0 }, 32),
    ("乱数の円盤 半径9", Seed::RandomDisk { radius: 9.0 }, 32),
    ("楕円形の塊", Seed::Ellipse, 32),
    ("細い輪", Seed::Ring, 32),
    (
        "S1s ゆらぎ 50%",
        Seed::Pattern {
            code: "S1s",
            orientation: Orientation::AsIs,
            noise: 0.5,
        },
        8,
    ),
    ("S1s 左半分を消す", Seed::S1sHalfErased, 8),
    ("S1s ぼかす", Seed::S1sBlurred, 8),
    (
        "O2u 総量をそろえる",
        Seed::MassMatched(&Seed::Pattern {
            code: "O2u",
            orientation: Orientation::AsIs,
            noise: 0.001,
        }),
        8,
    ),
    (
        "OG2g 総量をそろえる",
        Seed::MassMatched(&Seed::Pattern {
            code: "OG2g",
            orientation: Orientation::AsIs,
            noise: 0.001,
        }),
        8,
    ),
    (
        "2S1v 総量をそろえる",
        Seed::MassMatched(&Seed::Pattern {
            code: "2S1v",
            orientation: Orientation::AsIs,
            noise: 0.001,
        }),
        8,
    ),
    (
        "円盤5 総量をそろえる",
        Seed::MassMatched(&Seed::RandomDisk { radius: 5.0 }),
        32,
    ),
    (
        "円盤7 総量をそろえる",
        Seed::MassMatched(&Seed::RandomDisk { radius: 7.0 }),
        32,
    ),
    (
        "円盤9 総量をそろえる",
        Seed::MassMatched(&Seed::RandomDisk { radius: 9.0 }),
        32,
    ),
    ("楕円 総量をそろえる", Seed::MassMatched(&Seed::Ellipse), 32),
    ("輪 総量をそろえる", Seed::MassMatched(&Seed::Ring), 32),
];

/// 外部クレートに頼らない決定的な疑似乱数(xorshift64)。
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

    fn unit(&mut self) -> f32 {
        (self.next_u64() >> 11) as f32 / (1u64 << 53) as f32
    }

    fn range_f32(&mut self, low: f32, high: f32) -> f32 {
        low + self.unit() * (high - low)
    }
}

fn seed_for(label: &str, trial: u64) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in label.bytes().chain(trial.to_le_bytes()) {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash | 1
}

/// 向きを変えたパターンの、座標 (x, y) の値。
fn oriented_value(pattern: &Pattern, orientation: Orientation, x: usize, y: usize) -> f32 {
    let (w, h) = (pattern.width(), pattern.height());
    match orientation {
        Orientation::AsIs => pattern.get(x, y),
        // 90度回転した図の (x, y) は、元の図の (y, h - 1 - x)。回転後の幅は h・高さは w
        Orientation::Rotate90 => pattern.get(y, h - 1 - x),
        Orientation::Rotate180 => pattern.get(w - 1 - x, h - 1 - y),
        Orientation::MirrorX => pattern.get(w - 1 - x, y),
    }
}

fn oriented_size(pattern: &Pattern, orientation: Orientation) -> (usize, usize) {
    match orientation {
        Orientation::Rotate90 => (pattern.height(), pattern.width()),
        _ => (pattern.width(), pattern.height()),
    }
}

/// トーラス上で、場の中心 (16, 16) からの符号付きの差。
fn centered_offset(coordinate: usize) -> f32 {
    coordinate as f32 + 0.5 - FIELD_SIZE as f32 / 2.0
}

/// 初期状態の値の並び(行優先)。
fn initial_values(seed: Seed, animals: &[Animal], rng: &mut Rng) -> Vec<f32> {
    let mut values = vec![0.0f32; CELLS];
    match seed {
        Seed::Pattern {
            code,
            orientation,
            noise,
        } => {
            let animal = animals.iter().find(|a| a.code == code).unwrap();
            let (width, height) = oriented_size(&animal.pattern, orientation);
            let left = FIELD_SIZE.saturating_sub(width) / 2;
            let top = FIELD_SIZE.saturating_sub(height) / 2;
            for y in 0..height.min(FIELD_SIZE) {
                for x in 0..width.min(FIELD_SIZE) {
                    let value = oriented_value(&animal.pattern, orientation, x, y);
                    let jitter = 1.0 + rng.range_f32(-noise, noise);
                    values[(top + y) * FIELD_SIZE + left + x] = (value * jitter).clamp(0.0, 1.0);
                }
            }
        }
        Seed::RandomDisk { radius } => {
            for (index, value) in values.iter_mut().enumerate() {
                let (dx, dy) = (
                    centered_offset(index % FIELD_SIZE),
                    centered_offset(index / FIELD_SIZE),
                );
                let inside = (dx * dx + dy * dy).sqrt() <= radius;
                let random = rng.unit();
                if inside {
                    *value = random;
                }
            }
        }
        Seed::Ellipse => {
            let angle = rng.range_f32(0.0, TAU);
            let (cos, sin) = (angle.cos(), angle.sin());
            for (index, value) in values.iter_mut().enumerate() {
                let (dx, dy) = (
                    centered_offset(index % FIELD_SIZE),
                    centered_offset(index / FIELD_SIZE),
                );
                let (along, across) = (dx * cos + dy * sin, -dx * sin + dy * cos);
                let exponent = (along / 8.0).powi(2) + (across / 4.0).powi(2);
                let jitter = 1.0 + rng.range_f32(-0.05, 0.05);
                *value = ((-exponent).exp() * jitter).clamp(0.0, 1.0);
            }
        }
        Seed::Ring => {
            for (index, value) in values.iter_mut().enumerate() {
                let (dx, dy) = (
                    centered_offset(index % FIELD_SIZE),
                    centered_offset(index / FIELD_SIZE),
                );
                let distance = (dx * dx + dy * dy).sqrt();
                let random = rng.range_f32(0.6, 1.0);
                if (distance - 6.0).abs() <= 1.0 {
                    *value = random;
                }
            }
        }
        Seed::S1sHalfErased => {
            // パターンは場の中央に置くので、場の左半分を消せばパターンの左半分が消える
            values = initial_values(S1S_AS_IS, animals, rng);
            for (index, value) in values.iter_mut().enumerate() {
                if index % FIELD_SIZE < FIELD_SIZE / 2 {
                    *value = 0.0;
                }
            }
        }
        Seed::S1sBlurred => {
            values = initial_values(S1S_AS_IS, animals, rng);
            for _ in 0..2 {
                values = box_blur(&values);
            }
        }
        Seed::MassMatched(inner) => {
            values = initial_values(*inner, animals, rng);
            let s1s = animals.iter().find(|a| a.code == "S1s").unwrap();
            let target = pattern_mass(&s1s.pattern);
            for _ in 0..MASS_MATCH_ITERATIONS {
                let mass: f32 = values.iter().sum();
                if mass <= 1e-6 {
                    break;
                }
                let scale = target / mass;
                for value in &mut values {
                    *value = (*value * scale).min(1.0);
                }
            }
        }
    }
    values
}

fn initial_field(seed: Seed, animals: &[Animal], rng: &mut Rng) -> Field {
    let values = initial_values(seed, animals, rng);
    let mut field = Field::new(FIELD_SIZE, FIELD_SIZE);
    field.map(|x, y, _| values[y * FIELD_SIZE + x]);
    field
}

fn pattern_mass(pattern: &Pattern) -> f32 {
    (0..pattern.height())
        .flat_map(|y| (0..pattern.width()).map(move |x| (x, y)))
        .map(|(x, y)| pattern.get(x, y))
        .sum()
}

/// トーラス上の 3×3 の平均。
fn box_blur(values: &[f32]) -> Vec<f32> {
    let size = FIELD_SIZE as i32;
    (0..CELLS)
        .map(|index| {
            let (x, y) = ((index % FIELD_SIZE) as i32, (index / FIELD_SIZE) as i32);
            let mut total = 0.0;
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let nx = (x + dx).rem_euclid(size) as usize;
                    let ny = (y + dy).rem_euclid(size) as usize;
                    total += values[ny * FIELD_SIZE + nx];
                }
            }
            total / 9.0
        })
        .collect()
}

fn toroidal_offset(offset: f32) -> f32 {
    let size = FIELD_SIZE as f32;
    let offset = offset.rem_euclid(size);
    if offset > size / 2.0 {
        offset - size
    } else {
        offset
    }
}

fn visible_area(field: &Field) -> usize {
    let view = field.view();
    (0..CELLS)
        .filter(|index| view.get(index % FIELD_SIZE, index / FIELD_SIZE) > CLEARLY_VISIBLE)
        .count()
}

fn write_pgm(path: &Path, field: &Field) {
    let view = field.view();
    let scale = 4;
    let side = FIELD_SIZE * scale;
    let mut bytes = format!("P5\n{side} {side}\n255\n").into_bytes();
    for y in 0..side {
        for x in 0..side {
            let value = view.get(x / scale, y / scale).clamp(0.0, 1.0);
            bytes.push((value * 255.0).round() as u8);
        }
    }
    fs::write(path, bytes).expect("PGM を書き出せない");
}

/// ある時点の体の様子。
#[derive(Clone, Copy, PartialEq)]
enum State {
    Gliding,
    Stopped,
    InBetween,
    Collapsed,
    Exploded,
}

impl State {
    fn from_speed(speed: f32) -> Self {
        if speed > GLIDING_SPEED {
            Self::Gliding
        } else if speed < STOPPED_SPEED {
            Self::Stopped
        } else {
            Self::InBetween
        }
    }
}

#[derive(Clone, Copy)]
struct Outcome {
    /// 各測定時点の様子。壊れたら、それ以降の時点も壊れた様子で埋める。
    states: [State; 2],
    /// 各測定時点の (速さ, はっきり見える点の数の平均)。壊れた時点は 0。
    measures: [(f32, f32); 2],
    /// 初期状態の総量。総量をそろえる処理が効いているかを確かめるため。
    initial_mass: f32,
    /// 崩壊・膨張したステップ。壊れなければ `None`。
    broken_at: Option<u32>,
}

fn run(
    seed: Seed,
    label: &str,
    trial: u64,
    animals: &[Animal],
    lenia: &mut Lenia,
    output: &Path,
) -> Outcome {
    let mut rng = Rng(seed_for(label, trial));
    let mut field = initial_field(seed, animals, &mut rng);
    let mut outcome = Outcome {
        states: [State::Collapsed; 2],
        measures: [(0.0, 0.0); 2],
        initial_mass: field.mass(),
        broken_at: None,
    };
    let mut previous_centroid: Option<(f32, f32)> = None;
    let (mut path, mut moves, mut area_total, mut area_samples) = (0.0f32, 0u32, 0.0f32, 0u32);
    let mut checkpoint = 0;
    for step in 1..=TOTAL_STEPS {
        lenia.step(&mut field, 1.0);
        let broken = if field.mass() < COLLAPSE_MASS {
            Some(State::Collapsed)
        } else if visible_area(&field) > EXPLODED_AREA {
            Some(State::Exploded)
        } else {
            None
        };
        if let Some(state) = broken {
            outcome.broken_at = Some(step);
            for slot in checkpoint..CHECKPOINTS.len() {
                outcome.states[slot] = state;
            }
            break;
        }
        let measuring = step > CHECKPOINTS[checkpoint] - MEASURE_STEPS;
        if measuring {
            let centroid = field.view().toroidal_centroid();
            if let (Some(a), Some(b)) = (previous_centroid, centroid) {
                let (dx, dy) = (toroidal_offset(b.0 - a.0), toroidal_offset(b.1 - a.1));
                path += (dx * dx + dy * dy).sqrt();
                moves += 1;
            }
            previous_centroid = centroid;
            area_total += visible_area(&field) as f32;
            area_samples += 1;
        }
        if step == CHECKPOINTS[checkpoint] {
            let speed = path / moves.max(1) as f32;
            outcome.states[checkpoint] = State::from_speed(speed);
            outcome.measures[checkpoint] = (speed, area_total / area_samples.max(1) as f32);
            previous_centroid = None;
            (path, moves, area_total, area_samples) = (0.0, 0, 0.0, 0);
            checkpoint += 1;
            if checkpoint == CHECKPOINTS.len() {
                break;
            }
        }
    }
    if trial < SNAPSHOT_TRIALS {
        let name = format!("{}_{trial}.pgm", label.replace(' ', "_"));
        write_pgm(&output.join(name), &field);
    }
    outcome
}

fn count(outcomes: &[Outcome], checkpoint: usize, state: State) -> usize {
    outcomes
        .iter()
        .filter(|o| o.states[checkpoint] == state)
        .count()
}

fn main() {
    let output = PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("出力先のディレクトリを引数で渡す"),
    );
    fs::create_dir_all(&output).expect("出力先を作れない");
    let started = Instant::now();
    let animals: Vec<Animal> = ["S1s", "O2u", "OG2g", "2S1v"]
        .iter()
        .map(|code| load_animal(code).unwrap())
        .collect();
    let s1s_params = animals[0].params.clone();

    let jobs: Vec<(usize, u64)> = SEEDS
        .iter()
        .enumerate()
        .flat_map(|(index, (_, _, trials))| (0..*trials).map(move |trial| (index, trial)))
        .collect();
    let job_count = jobs.len();
    let queue = Mutex::new(jobs.into_iter().enumerate());
    let results: Mutex<Vec<Option<(usize, Outcome)>>> =
        Mutex::new((0..job_count).map(|_| None).collect());
    let workers = thread::available_parallelism().map_or(4, |n| n.get());
    thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                let mut lenia = Lenia::new(s1s_params.clone());
                loop {
                    let Some((index, (seed_index, trial))) = queue.lock().unwrap().next() else {
                        break;
                    };
                    let (label, seed, _) = SEEDS[seed_index];
                    let outcome = run(seed, label, trial, &animals, &mut lenia, &output);
                    results.lock().unwrap()[index] = Some((seed_index, outcome));
                }
            });
        }
    });
    let results: Vec<(usize, Outcome)> = results
        .into_inner()
        .unwrap()
        .into_iter()
        .map(Option::unwrap)
        .collect();

    println!(
        "S1s の規則、場 {FIELD_SIZE}×{FIELD_SIZE}、成長の強さ 1.0。速さ > {GLIDING_SPEED} を滑る、< {STOPPED_SPEED} を止まるとする"
    );
    println!(
        "総量をそろえる目標(S1s のパターンの総量): {:.1}",
        pattern_mass(&animals[0].pattern)
    );
    println!(
        "  {:18} | 回数 | 3000: 滑る 止まる 間 崩壊 膨張 | 6000: 滑る 止まる 間 崩壊 膨張 | 滑る→止まる 止まる→滑る | 6000 の速さ(滑る) 点の数(止まる) | 初期の総量(平均) 壊れたステップ(中央値)",
        "初期状態"
    );
    for (seed_index, (label, _, trials)) in SEEDS.iter().enumerate() {
        let outcomes: Vec<&Outcome> = results
            .iter()
            .filter(|(index, _)| *index == seed_index)
            .map(|(_, outcome)| outcome)
            .collect();
        let owned: Vec<Outcome> = outcomes.iter().map(|o| **o).collect();
        let cells = |checkpoint: usize| {
            format!(
                "{:>4} {:>6} {:>2} {:>4} {:>4}",
                count(&owned, checkpoint, State::Gliding),
                count(&owned, checkpoint, State::Stopped),
                count(&owned, checkpoint, State::InBetween),
                count(&owned, checkpoint, State::Collapsed),
                count(&owned, checkpoint, State::Exploded)
            )
        };
        let glide_to_stop = owned
            .iter()
            .filter(|o| o.states == [State::Gliding, State::Stopped])
            .count();
        let stop_to_glide = owned
            .iter()
            .filter(|o| o.states == [State::Stopped, State::Gliding])
            .count();
        let mean_of = |state: State, pick: fn(&(f32, f32)) -> f32| {
            let values: Vec<f32> = owned
                .iter()
                .filter(|o| o.states[1] == state)
                .map(|o| pick(&o.measures[1]))
                .collect();
            if values.is_empty() {
                "-".to_string()
            } else {
                format!("{:.3}", values.iter().sum::<f32>() / values.len() as f32)
            }
        };
        let initial_mass =
            owned.iter().map(|o| o.initial_mass).sum::<f32>() / owned.len().max(1) as f32;
        let mut broken_steps: Vec<u32> = owned.iter().filter_map(|o| o.broken_at).collect();
        broken_steps.sort_unstable();
        let broken_median = broken_steps
            .get(broken_steps.len() / 2)
            .map_or("-".to_string(), |step| step.to_string());
        println!(
            "  {label:18} | {trials:>4} | {} | {} | {glide_to_stop:>11} {stop_to_glide:>11} | {} {} | {initial_mass:.1} {broken_median}",
            cells(0),
            cells(1),
            mean_of(State::Gliding, |m| m.0),
            mean_of(State::Stopped, |m| m.1)
        );
    }
    println!();
    println!("合計 {:.0} 秒", started.elapsed().as_secs_f32());
}
