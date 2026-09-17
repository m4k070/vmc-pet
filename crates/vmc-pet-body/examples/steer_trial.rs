//! 【実験】突く位置を選ぶだけで、体の進む向きを変えられるか。
//!
//! 自律コントローラの表現を増やす候補として、「触れられた方へ寄る/離れる」のような向きの表現を
//! 考えた。いまのコントローラは形の偏りをならす向きに軽く突くだけで、どこを突くかで体の動きを
//! 導けるかは測っていない。そこで、突く強さはいまの検証済みの範囲(コントローラの強さと、クリックの
//! 強さ)に留めたまま、目標の向きに対して前側/後ろ側を突き続け、目標の向きへどれだけ進むかを測る。
//!
//! - 生物: 同梱の4種。場はペットと同じ 32×32、成長の強さ 1.0(元気な体)、テンポ 1.0
//! - 出発点: パターンを置いて 900 + 150·k ステップ育てた状態(k = 0..12。向きと位相が変わる)
//! - 目標の向き: 8方位
//! - 突き方: 重心から目標の向きに `offset` 離れた点(前側)に、一定のステップごとに山型の摂動を
//!   注入する。後ろ側を突くのは、反対の目標の前側を突くのと同じなので、8方位の中に含まれる
//!   (進みの符号が逆になるだけ)。比較として、何もしない場合と、いまのコントローラの
//!   場合も測る(この2つは目標に依らないので、同じ試行の変位を8方位に射影する)
//! - 測る量: 900 ステップ(60 秒)の重心の変位を目標の向きへ射影した「進み」、変位と目標のなす角の
//!   余弦、崩壊(総量 5 未満)・膨張(はっきり見える点が場の半分超)
//!
//! 使い方: `cargo run --release -p vmc-pet-body --example steer_trial`。ペット本体の振る舞いには触れない。

use std::f32::consts::TAU;
use std::sync::Mutex;
use std::thread;
use std::time::Instant;

use vmc_pet_body::controller::{AutonomousController, Observation};
use vmc_pet_body::{list_animals, load_animal, Animal, CellPos, Field, Lenia, Perturbation};

const FIELD_SIZE: usize = 32;
const CELLS: usize = FIELD_SIZE * FIELD_SIZE;
const COLLAPSE_MASS: f32 = 5.0;
const CLEARLY_VISIBLE: f32 = 0.2;
const EXPLODED_AREA: usize = CELLS / 2;

const SETTLE_STEPS: u32 = 900;
const SETTLE_SPACING: u32 = 150;
const STARTS: u32 = 12;
const DIRECTIONS: usize = 8;
const RUN_STEPS: u32 = 900;

/// 突く強さ。どちらもペットで安全を確かめた値。
#[derive(Clone, Copy, Debug)]
struct Strength {
    label: &'static str,
    amount: f32,
    radius: f32,
    every_steps: u32,
}

/// いまのコントローラの既定値(`ControllerParams::default`)。
const GENTLE: Strength = Strength {
    label: "コントローラの強さ",
    amount: 0.0239,
    radius: 4.7612,
    every_steps: 17,
};

/// 中間の強さ。量はクリックの 0.4 倍、頻度はコントローラと同じ。
const MIDDLE: Strength = Strength {
    label: "中間の強さ",
    amount: 0.08,
    radius: 4.5,
    every_steps: 17,
};

/// クリックの強さ(`touch.rs`)。クリックは連打されうるが、ここでは 2 秒に1回に留める。
const CLICK: Strength = Strength {
    label: "クリックの強さ",
    amount: 0.20,
    radius: 4.5,
    every_steps: 30,
};

#[derive(Clone, Copy, Debug)]
enum Condition {
    Nothing,
    Controller,
    /// 重心から目標の向きへ `offset` 離れた点(前側)を突く。
    Steer {
        strength: Strength,
        offset: f32,
    },
}

impl Condition {
    fn label(self) -> String {
        match self {
            Self::Nothing => "何もしない".to_string(),
            Self::Controller => "いまのコントローラ".to_string(),
            Self::Steer { strength, offset } => {
                format!("前側を突く・{}・距離 {offset}", strength.label)
            }
        }
    }

    fn depends_on_direction(self) -> bool {
        matches!(self, Self::Steer { .. })
    }
}

const CONDITIONS: [Condition; 8] = [
    Condition::Nothing,
    Condition::Controller,
    Condition::Steer {
        strength: GENTLE,
        offset: 4.3,
    },
    Condition::Steer {
        strength: GENTLE,
        offset: 8.0,
    },
    Condition::Steer {
        strength: MIDDLE,
        offset: 4.3,
    },
    Condition::Steer {
        strength: MIDDLE,
        offset: 8.0,
    },
    Condition::Steer {
        strength: CLICK,
        offset: 4.3,
    },
    Condition::Steer {
        strength: CLICK,
        offset: 8.0,
    },
];

fn toroidal_offset(offset: f32) -> f32 {
    let size = FIELD_SIZE as f32;
    let wrapped = offset.rem_euclid(size);
    if wrapped > size / 2.0 {
        wrapped - size
    } else {
        wrapped
    }
}

fn visible_area(field: &Field) -> usize {
    let view = field.view();
    (0..FIELD_SIZE)
        .flat_map(|y| (0..FIELD_SIZE).map(move |x| (x, y)))
        .filter(|&(x, y)| view.get(x, y) > CLEARLY_VISIBLE)
        .count()
}

fn copy_field(source: &Field) -> Field {
    let view = source.view();
    let mut field = Field::new(FIELD_SIZE, FIELD_SIZE);
    field.map(|x, y, _| view.get(x, y));
    field
}

fn settled_field(animal: &Animal, start: u32) -> Field {
    let mut lenia = Lenia::new(animal.params.clone());
    let mut field = Field::new(FIELD_SIZE, FIELD_SIZE);
    field.place_centered(&animal.pattern);
    for _ in 0..SETTLE_STEPS + SETTLE_SPACING * start {
        lenia.step(&mut field, 1.0);
    }
    field
}

/// 1回の試行の結果。
#[derive(Clone, Copy)]
struct Run {
    /// 重心の変位の合計(折り返しをほどいたもの)。
    displacement: (f32, f32),
    broken: bool,
}

fn run(animal: &Animal, start: &Field, condition: Condition, direction: (f32, f32)) -> Run {
    let mut lenia = Lenia::new(animal.params.clone());
    let mut field = copy_field(start);
    let mut controller = AutonomousController::new();
    let mut previous = field.view().toroidal_centroid();
    let mut displacement = (0.0, 0.0);
    for step in 1..=RUN_STEPS {
        match condition {
            Condition::Nothing => {}
            Condition::Controller => {
                let observation = Observation {
                    field: field.view(),
                    energy: 1.0,
                };
                if let Some(perturbation) = controller.maybe_act(observation) {
                    field.inject(&perturbation);
                }
            }
            Condition::Steer { strength, offset } => {
                let centroid = if step % strength.every_steps == 0 {
                    field.view().toroidal_centroid()
                } else {
                    None
                };
                if let Some((cx, cy)) = centroid {
                    let size = FIELD_SIZE as f32;
                    let x = (cx + direction.0 * offset).rem_euclid(size);
                    let y = (cy + direction.1 * offset).rem_euclid(size);
                    field.inject(&Perturbation {
                        at: CellPos {
                            x: (x as usize).min(FIELD_SIZE - 1),
                            y: (y as usize).min(FIELD_SIZE - 1),
                        },
                        radius: strength.radius,
                        amount: strength.amount,
                    });
                }
            }
        }
        lenia.step(&mut field, 1.0);
        if field.mass() < COLLAPSE_MASS || visible_area(&field) > EXPLODED_AREA {
            return Run {
                displacement,
                broken: true,
            };
        }
        let current = field.view().toroidal_centroid();
        if let (Some(a), Some(b)) = (previous, current) {
            displacement.0 += toroidal_offset(b.0 - a.0);
            displacement.1 += toroidal_offset(b.1 - a.1);
        }
        previous = current;
    }
    Run {
        displacement,
        broken: false,
    }
}

fn direction(index: usize) -> (f32, f32) {
    let angle = TAU * index as f32 / DIRECTIONS as f32;
    (angle.cos(), angle.sin())
}

/// 条件ごとの集計。
#[derive(Default)]
struct Tally {
    /// 出発点ごとの余弦の和と数。出発点ごとに8方位で平均すると、元の進行方向の偏りが消える。
    cosine_by_start: Vec<(f32, usize)>,
    progress: Vec<f32>,
    cosine: Vec<f32>,
    distance: Vec<f32>,
    broken: usize,
    runs: usize,
}

fn mean(values: &[f32]) -> f32 {
    values.iter().sum::<f32>() / values.len().max(1) as f32
}

fn standard_deviation(values: &[f32]) -> f32 {
    let m = mean(values);
    (values.iter().map(|v| (v - m) * (v - m)).sum::<f32>() / values.len().max(1) as f32).sqrt()
}

fn main() {
    let started = Instant::now();
    let codes: Vec<String> = list_animals()
        .expect("animals.json を読めない")
        .into_iter()
        .map(|(code, _)| code)
        .collect();
    println!(
        "場 {FIELD_SIZE}×{FIELD_SIZE}、出発点 {STARTS} 通り × 目標 {DIRECTIONS} 方位、{RUN_STEPS} ステップ(60 秒)"
    );
    println!("進み = 変位を目標の向きへ射影した長さ(セル)。余弦 = 変位と目標のなす角の余弦。\n");

    for code in codes {
        let animal = load_animal(&code).expect("生物を読めない");
        let starts: Vec<Field> = (0..STARTS).map(|k| settled_field(&animal, k)).collect();

        // (条件, 出発点, 方位) の組を並列に走らせる。目標に依らない条件は方位 0 だけ走らせる。
        let mut jobs = Vec::new();
        for (c, condition) in CONDITIONS.iter().enumerate() {
            for s in 0..STARTS as usize {
                let directions = if condition.depends_on_direction() {
                    DIRECTIONS
                } else {
                    1
                };
                for d in 0..directions {
                    jobs.push((c, s, d));
                }
            }
        }
        let jobs = Mutex::new(jobs);
        let results = Mutex::new(Vec::new());
        thread::scope(|scope| {
            for _ in 0..thread::available_parallelism().map_or(4, |n| n.get()) {
                scope.spawn(|| loop {
                    let Some((c, s, d)) = jobs.lock().unwrap().pop() else {
                        break;
                    };
                    let outcome = run(&animal, &starts[s], CONDITIONS[c], direction(d));
                    results.lock().unwrap().push((c, s, d, outcome));
                });
            }
        });

        let mut tallies: Vec<Tally> = CONDITIONS.iter().map(|_| Tally::default()).collect();
        for (c, s, d, outcome) in results.into_inner().unwrap() {
            let tally = &mut tallies[c];
            // 目標に依らない条件は、同じ変位を8方位すべての目標に射影する
            let targets: Vec<usize> = if CONDITIONS[c].depends_on_direction() {
                vec![d]
            } else {
                (0..DIRECTIONS).collect()
            };
            tally.runs += 1;
            if tally.cosine_by_start.is_empty() {
                tally.cosine_by_start = vec![(0.0, 0); STARTS as usize];
            }
            if outcome.broken {
                tally.broken += 1;
                continue;
            }
            let (dx, dy) = outcome.displacement;
            let distance = (dx * dx + dy * dy).sqrt();
            tally.distance.push(distance);
            for target in targets {
                let (ux, uy) = direction(target);
                let progress = dx * ux + dy * uy;
                tally.progress.push(progress);
                let cosine = if distance > 1e-3 {
                    progress / distance
                } else {
                    0.0
                };
                tally.cosine.push(cosine);
                tally.cosine_by_start[s].0 += cosine;
                tally.cosine_by_start[s].1 += 1;
            }
        }

        println!("## {} ({})", animal.code, animal.name);
        println!("| 条件 | 進み 平均±標準偏差 | 余弦 平均 | 出発点ごとの余弦 最小〜最大 | 動いた距離 平均 | 壊れた |");
        println!("|---|---|---|---|---|---|");
        for (condition, tally) in CONDITIONS.iter().zip(&tallies) {
            let by_start: Vec<f32> = tally
                .cosine_by_start
                .iter()
                .filter(|(_, n)| *n > 0)
                .map(|(sum, n)| sum / *n as f32)
                .collect();
            let lowest = by_start.iter().copied().fold(f32::INFINITY, f32::min);
            let highest = by_start.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            println!(
                "| {} | {:+.1} ± {:.1} | {:+.2} | {:+.2}〜{:+.2} | {:.1} | {}/{} |",
                condition.label(),
                mean(&tally.progress),
                standard_deviation(&tally.progress),
                mean(&tally.cosine),
                lowest,
                highest,
                mean(&tally.distance),
                tally.broken,
                tally.runs
            );
        }
        println!();
    }
    println!("所要時間 {:.1} 秒", started.elapsed().as_secs_f32());
}
