//! 【実験】表現軸の少なさを、Glaberish の「生き残る関数」で補えるか。
//!
//! いまの表現は色(色素)・テンポ(活発さ)・身じろぎ(echo)で、テンポが作る違いは活発さの強弱
//! だけだった(docs/DESIGN.md「3状態をテンポに割り当てて直接測った」)。成長関数を「生まれる
//! 関数」G と「生き残る関数」P に分けると、体の内側(値の大きいセル)だけに効くつまみができる。
//! これで、活発さとは質の違う見た目(明るさ・大きさ・中の抜け)を、崩壊させずに作れて、元に
//! 戻せるかを確かめる(docs/DESIGN.md「生き残る関数で表情を足せるか」)。
//!
//! 条件の作り方は「体側の表情の軸を振り分けた」に合わせる:
//!
//! 1. 落ち着かせる(長さを乱数でずらし、場にごく小さなゆらぎを掛けて試行ごとに軌跡を変える)
//! 2. エネルギーを徐々に尽きさせる(成長の強さ 1.0 → 下限。元気な体は 1.0 のまま)
//! 3. 表情をかける: P(または比べるための G)の中心と幅を、徐々に目標まで動かす
//! 4. 保つ。最後の 900 ステップ(60秒)で見た目の特徴を測る
//! 5. 元に戻す: 関数を徐々にその生物の成長関数へ戻す
//! 6. 保つ。最後の 900 ステップで特徴を測り、表情をかけなかった試行と比べる
//!
//! 見た目の特徴は、人がドットの画面で見て分かるものに限る: 移動の速さ・脈動(いまの
//! `legibility` と同じ)、はっきり見える点の数(大きさ)、体に囲まれた暗いセルの数(中の抜け)。
//! 数字は形の違いを捉えきれないので、試行0の姿を画像に書き出して目でも確かめる。
//!
//! 使い方: `cargo run --release -p vmc-pet-body --example persistence_expression -- <出力先>`。
//! 出力先に試行0の最後の姿を PGM で書き出す。ペット本体の振る舞いには触れない。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread;
use std::time::Instant;

use vmc_pet_body::{load_animal, Animal, Field, GrowthFunction, Lenia};

const FIELD_SIZE: usize = 32;
const CELLS: usize = FIELD_SIZE * FIELD_SIZE;
/// `Pet::step` と同じ崩壊の判定。
const COLLAPSE_MASS: f32 = 5.0;
/// はっきり見える点とみなす値。点の半径は値の平方根、不透明度は値そのもので描くので、
/// 0.2 で半径は最大の約45%・不透明度20%になる(描画を始める 0.004 はほぼ見えない)。
const CLEARLY_VISIBLE: f32 = 0.2;
/// はっきり見える点が場の半分を超えたら、体ではなく場を埋めた「膨張」とみなす。
const EXPLODED_AREA: usize = CELLS / 2;

const MAX_WARMUP_JITTER: u32 = 300;
const NOISE: f32 = 1e-3;
/// 触られないままエネルギーが尽きるまでのステップ数(`LeniaBody` の減り方と同じ約150秒)。
const NEGLECT_STEPS: u32 = 2_250;
const RAMP_STEPS: u32 = 750;
const HOLD_STEPS: u32 = 3_000;
/// 見た目の特徴を測る長さ(60秒)。保つ区間の最後に置く。
const MEASURE_STEPS: u32 = 900;
/// 動きのむらを見る区切りと同じ1秒。穴を数える間隔にも使う(毎ステップ数えるほど変わらない)。
const HOLE_SAMPLE_INTERVAL: u32 = 15;
const TRIALS: u64 = 4;

/// エネルギーの下限。元気(1.0)と、放置されて弱りきった体(`MIN_GROWTH_SCALE` と同じ 0.85)。
const ENERGY_FLOORS: [f32; 2] = [1.0, 0.85];

/// 関数のどちらを動かすか。
#[derive(Clone, Copy, PartialEq)]
enum Target {
    None,
    Persistence,
    Genesis,
}

/// 表情の条件。中心は成長関数の幅 σ を単位にずらし、幅は倍率で変える。
#[derive(Clone, Copy)]
struct Expression {
    label: &'static str,
    target: Target,
    center_shift: f32,
    width_ratio: f32,
}

const EXPRESSIONS: [Expression; 17] = [
    Expression {
        label: "そのまま",
        target: Target::None,
        center_shift: 0.0,
        width_ratio: 1.0,
    },
    Expression {
        label: "P 中心 -1σ",
        target: Target::Persistence,
        center_shift: -1.0,
        width_ratio: 1.0,
    },
    Expression {
        label: "P 中心 -0.5σ",
        target: Target::Persistence,
        center_shift: -0.5,
        width_ratio: 1.0,
    },
    Expression {
        label: "P 中心 -0.25σ",
        target: Target::Persistence,
        center_shift: -0.25,
        width_ratio: 1.0,
    },
    Expression {
        label: "P 中心 +0.25σ",
        target: Target::Persistence,
        center_shift: 0.25,
        width_ratio: 1.0,
    },
    Expression {
        label: "P 中心 +0.5σ",
        target: Target::Persistence,
        center_shift: 0.5,
        width_ratio: 1.0,
    },
    Expression {
        label: "P 中心 +1σ",
        target: Target::Persistence,
        center_shift: 1.0,
        width_ratio: 1.0,
    },
    Expression {
        label: "P 幅 ×0.7",
        target: Target::Persistence,
        center_shift: 0.0,
        width_ratio: 0.7,
    },
    Expression {
        label: "P 幅 ×0.85",
        target: Target::Persistence,
        center_shift: 0.0,
        width_ratio: 0.85,
    },
    Expression {
        label: "P 幅 ×0.93",
        target: Target::Persistence,
        center_shift: 0.0,
        width_ratio: 0.93,
    },
    Expression {
        label: "P 幅 ×1.1",
        target: Target::Persistence,
        center_shift: 0.0,
        width_ratio: 1.1,
    },
    Expression {
        label: "P 幅 ×1.2",
        target: Target::Persistence,
        center_shift: 0.0,
        width_ratio: 1.2,
    },
    Expression {
        label: "P 幅 ×1.5",
        target: Target::Persistence,
        center_shift: 0.0,
        width_ratio: 1.5,
    },
    Expression {
        label: "G 中心 -1σ",
        target: Target::Genesis,
        center_shift: -1.0,
        width_ratio: 1.0,
    },
    Expression {
        label: "G 中心 +1σ",
        target: Target::Genesis,
        center_shift: 1.0,
        width_ratio: 1.0,
    },
    Expression {
        label: "G 幅 ×0.7",
        target: Target::Genesis,
        center_shift: 0.0,
        width_ratio: 0.7,
    },
    Expression {
        label: "G 幅 ×1.5",
        target: Target::Genesis,
        center_shift: 0.0,
        width_ratio: 1.5,
    },
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

    fn range_u32(&mut self, low: u32, high: u32) -> u32 {
        low + (self.next_u64() % (high - low + 1) as u64) as u32
    }
}

/// 生物と試行番号だけから種を作る。表情の条件によらず同じ種にして、表情をかけるまでの
/// 軌跡を「そのまま」の試行とそろえる。
fn seed_for(code: &str, trial: u64) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in code.bytes().chain(trial.to_le_bytes()) {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash | 1
}

/// `base` の中心を `shift` σ・幅を `ratio` 倍へ、`progress`(0..=1)の割合だけ動かした関数。
fn shifted(base: GrowthFunction, shift: f32, ratio: f32, progress: f32) -> GrowthFunction {
    GrowthFunction {
        mapping: base.mapping,
        center: base.center + shift * base.width * progress,
        width: base.width * (1.0 + (ratio - 1.0) * progress),
    }
}

/// 表情を `progress` の割合だけかけたときの (生まれる関数, 生き残る関数)。
fn functions_for(
    base: GrowthFunction,
    expression: Expression,
    progress: f32,
) -> (GrowthFunction, GrowthFunction) {
    let moved = shifted(
        base,
        expression.center_shift,
        expression.width_ratio,
        progress,
    );
    match expression.target {
        Target::None => (base, base),
        Target::Persistence => (base, moved),
        Target::Genesis => (moved, base),
    }
}

/// 60秒ぶんの見た目の特徴。
#[derive(Clone, Copy, Default)]
struct Features {
    /// 1ステップあたりの重心の移動量。
    mean_speed: f32,
    /// 総量の標準偏差。
    mass_deviation: f32,
    mean_mass: f32,
    /// はっきり見える点の数の平均。
    visible_area: f32,
    /// 体に囲まれた暗いセルの数の平均。尾の輪が体と囲む暗がりも数えるので、数だけで
    /// 「中が抜けた」とは言えない(スナップショットで確かめる)。
    hole_cells: f32,
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

/// 暗いセル(はっきり見えない)のうち、いちばん大きな暗い領域(背景)につながらないものの数。
/// 上下左右のつながりで、場は端で折り返す。
fn hole_cells(field: &Field) -> usize {
    let view = field.view();
    let dark = |index: usize| view.get(index % FIELD_SIZE, index / FIELD_SIZE) <= CLEARLY_VISIBLE;
    let mut seen = [false; CELLS];
    let mut region_sizes = Vec::new();
    for start in 0..CELLS {
        if seen[start] || !dark(start) {
            continue;
        }
        seen[start] = true;
        let mut stack = vec![start];
        let mut size = 0;
        while let Some(index) = stack.pop() {
            size += 1;
            let (x, y) = (index % FIELD_SIZE, index / FIELD_SIZE);
            let neighbours = [
                ((x + 1) % FIELD_SIZE, y),
                ((x + FIELD_SIZE - 1) % FIELD_SIZE, y),
                (x, (y + 1) % FIELD_SIZE),
                (x, (y + FIELD_SIZE - 1) % FIELD_SIZE),
            ];
            for (nx, ny) in neighbours {
                let neighbour = ny * FIELD_SIZE + nx;
                if !seen[neighbour] && dark(neighbour) {
                    seen[neighbour] = true;
                    stack.push(neighbour);
                }
            }
        }
        region_sizes.push(size);
    }
    let background = region_sizes.iter().copied().max().unwrap_or(0);
    region_sizes.iter().sum::<usize>() - background
}

fn visible_area(field: &Field) -> usize {
    let view = field.view();
    (0..CELLS)
        .filter(|index| view.get(index % FIELD_SIZE, index / FIELD_SIZE) > CLEARLY_VISIBLE)
        .count()
}

/// 特徴を測りながら1ステップずつ積み上げる。
#[derive(Default)]
struct FeatureRecorder {
    previous_centroid: Option<(f32, f32)>,
    path: f32,
    moves: u32,
    masses: Vec<f32>,
    area_total: f32,
    hole_total: f32,
    hole_samples: u32,
}

impl FeatureRecorder {
    fn record(&mut self, field: &Field, step: u32) {
        let centroid = field.view().toroidal_centroid();
        if let (Some(a), Some(b)) = (self.previous_centroid, centroid) {
            let (dx, dy) = (toroidal_offset(b.0 - a.0), toroidal_offset(b.1 - a.1));
            self.path += (dx * dx + dy * dy).sqrt();
            self.moves += 1;
        }
        self.previous_centroid = centroid;
        self.masses.push(field.mass());
        self.area_total += visible_area(field) as f32;
        if step.is_multiple_of(HOLE_SAMPLE_INTERVAL) {
            self.hole_total += hole_cells(field) as f32;
            self.hole_samples += 1;
        }
    }

    fn finish(&self) -> Features {
        let count = self.masses.len().max(1) as f32;
        let mean_mass = self.masses.iter().sum::<f32>() / count;
        let variance = self
            .masses
            .iter()
            .map(|mass| (mass - mean_mass) * (mass - mean_mass))
            .sum::<f32>()
            / count;
        Features {
            mean_speed: self.path / self.moves.max(1) as f32,
            mass_deviation: variance.sqrt(),
            mean_mass,
            visible_area: self.area_total / count,
            hole_cells: self.hole_total / self.hole_samples.max(1) as f32,
        }
    }
}

/// (区間の名前, 長さ, 区間の中のステップから表情をかける割合を返す関数)。
type Phase = (&'static str, u32, fn(u32) -> f32);

/// 試行の終わり方。
enum Fate {
    Survived,
    Collapsed { phase: &'static str },
    Exploded { phase: &'static str },
}

struct Outcome {
    fate: Fate,
    /// 表情をかけて保った最後の60秒。
    expressed: Features,
    /// 元に戻して保った最後の60秒。
    returned: Features,
}

fn relative_difference(a: f32, b: f32) -> f32 {
    let scale = a.abs().max(b.abs());
    if scale <= 1e-6 {
        0.0
    } else {
        (a - b).abs() / scale
    }
}

/// いまの `fitness::legibility` と同じ式(速さと脈動の相対差の平均)。
fn motion_difference(a: &Features, b: &Features) -> f32 {
    (relative_difference(a.mean_speed, b.mean_speed)
        + relative_difference(a.mass_deviation, b.mass_deviation))
        / 2.0
}

/// 大きさ(はっきり見える点の数)の相対差。いちばん明るい点(芯の明るさ)も最初は入れて
/// いたが、どの条件でも 1.0 に張り付いて違いを表さなかったので外した。
fn shape_difference(a: &Features, b: &Features) -> f32 {
    relative_difference(a.visible_area, b.visible_area)
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

struct Run<'a> {
    animal: &'a Animal,
    energy_floor: f32,
    expression: Expression,
    trial: u64,
    output: Option<&'a Path>,
}

impl Run<'_> {
    fn snapshot(&self, field: &Field, moment: &str) {
        let Some(output) = self.output else {
            return;
        };
        let name = format!(
            "{}_e{:.2}_{}_{moment}.pgm",
            self.animal.code,
            self.energy_floor,
            file_label(self.expression)
        );
        write_pgm(&output.join(name), field);
    }

    fn execute(&self) -> Outcome {
        let base = self.animal.params.growth_function();
        let mut rng = Rng(seed_for(&self.animal.code, self.trial));
        let mut lenia = Lenia::new(self.animal.params.clone());
        let mut field = Field::new(FIELD_SIZE, FIELD_SIZE);
        field.place_centered(&self.animal.pattern);
        for _ in 0..rng.range_u32(60, 60 + MAX_WARMUP_JITTER) {
            lenia.step_glaberish(&mut field, base, base, 1.0, 1.0);
        }
        let noise: Vec<f32> = (0..CELLS)
            .map(|_| 1.0 + rng.range_f32(-NOISE, NOISE))
            .collect();
        field.map(|x, y, value| (value * noise[y * FIELD_SIZE + x]).min(1.0));

        let mut outcome = Outcome {
            fate: Fate::Survived,
            expressed: Features::default(),
            returned: Features::default(),
        };
        let phases: [Phase; 5] = [
            ("エネルギーを尽きさせる", NEGLECT_STEPS, |_| 0.0),
            ("表情をかける", RAMP_STEPS, |step| {
                step as f32 / RAMP_STEPS as f32
            }),
            ("表情を保つ", HOLD_STEPS, |_| 1.0),
            ("元に戻す", RAMP_STEPS, |step| {
                1.0 - step as f32 / RAMP_STEPS as f32
            }),
            ("戻して保つ", HOLD_STEPS, |_| 0.0),
        ];
        let mut neglected_steps = 0u32;
        for (phase, length, progress_at) in phases {
            let mut recorder = FeatureRecorder::default();
            for step in 0..length {
                let depletion = (neglected_steps as f32 / NEGLECT_STEPS as f32).min(1.0);
                neglected_steps += 1;
                let growth_scale = 1.0 - (1.0 - self.energy_floor) * depletion;
                let (genesis, persistence) =
                    functions_for(base, self.expression, progress_at(step));
                lenia.step_glaberish(&mut field, genesis, persistence, growth_scale, 1.0);
                if field.mass() < COLLAPSE_MASS {
                    outcome.fate = Fate::Collapsed { phase };
                    return outcome;
                }
                if visible_area(&field) > EXPLODED_AREA {
                    outcome.fate = Fate::Exploded { phase };
                    return outcome;
                }
                if length == HOLD_STEPS && step >= HOLD_STEPS - MEASURE_STEPS {
                    recorder.record(&field, step);
                }
            }
            match phase {
                "表情を保つ" => {
                    outcome.expressed = recorder.finish();
                    self.snapshot(&field, "1expressed");
                }
                "戻して保つ" => {
                    outcome.returned = recorder.finish();
                    self.snapshot(&field, "2returned");
                }
                _ => {}
            }
        }
        outcome
    }
}

fn file_label(expression: Expression) -> String {
    let target = match expression.target {
        Target::None => "base",
        Target::Persistence => "P",
        Target::Genesis => "G",
    };
    format!(
        "{target}_c{:+.1}_w{:.2}",
        expression.center_shift, expression.width_ratio
    )
}

struct Job {
    code: String,
    energy_index: usize,
    expression_index: usize,
    trial: u64,
}

fn mean(values: impl Iterator<Item = f32>) -> Option<f32> {
    let (total, count) = values.fold((0.0, 0), |(t, c), v| (t + v, c + 1));
    (count > 0).then(|| total / count as f32)
}

fn format_mean(value: Option<f32>) -> String {
    value.map_or("   -".to_string(), |v| format!("{v:.2}"))
}

fn main() {
    let output = PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("出力先のディレクトリを引数で渡す"),
    );
    fs::create_dir_all(&output).expect("出力先を作れない");
    let started = Instant::now();
    let animals: Vec<Animal> = vmc_pet_body::list_animals()
        .unwrap()
        .into_iter()
        .map(|(code, _)| load_animal(&code).unwrap())
        .collect();

    let mut jobs = Vec::new();
    for animal in &animals {
        for energy_index in 0..ENERGY_FLOORS.len() {
            for expression_index in 0..EXPRESSIONS.len() {
                for trial in 0..TRIALS {
                    jobs.push(Job {
                        code: animal.code.clone(),
                        energy_index,
                        expression_index,
                        trial,
                    });
                }
            }
        }
    }
    let job_count = jobs.len();
    let queue = Mutex::new(jobs.into_iter().enumerate());
    let results: Mutex<Vec<Option<Outcome>>> = Mutex::new((0..job_count).map(|_| None).collect());
    let workers = thread::available_parallelism().map_or(4, |n| n.get());
    thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let Some((index, job)) = queue.lock().unwrap().next() else {
                    break;
                };
                let animal = animals.iter().find(|a| a.code == job.code).unwrap();
                let run = Run {
                    animal,
                    energy_floor: ENERGY_FLOORS[job.energy_index],
                    expression: EXPRESSIONS[job.expression_index],
                    trial: job.trial,
                    output: (job.trial == 0).then_some(output.as_path()),
                };
                let outcome = run.execute();
                results.lock().unwrap()[index] = Some(outcome);
            });
        }
    });
    let results: Vec<Outcome> = results
        .into_inner()
        .unwrap()
        .into_iter()
        .map(Option::unwrap)
        .collect();
    let per_trial = |animal_index: usize, energy_index: usize, expression_index: usize| {
        let start = ((animal_index * ENERGY_FLOORS.len() + energy_index) * EXPRESSIONS.len()
            + expression_index)
            * TRIALS as usize;
        &results[start..start + TRIALS as usize]
    };

    println!(
        "場 {FIELD_SIZE}×{FIELD_SIZE}、各条件 {TRIALS} 回。特徴は保った最後の {MEASURE_STEPS} ステップの平均。"
    );
    println!("  動き差 = 速さと脈動の相対差の平均(いまの legibility と同じ式)");
    println!("  形差   = はっきり見える点の数(値 > {CLEARLY_VISIBLE})の相対差");
    println!(
        "  いずれも同じ試行番号の「そのまま」と比べた平均。ゆらぎ = 「そのまま」の試行どうしの差"
    );
    println!(
        "  残り   = 同じ試行の、表情を保ったときと戻した後の差(小さいほど表情のまま戻っていない)"
    );
    println!();
    for (animal_index, animal) in animals.iter().enumerate() {
        let base = animal.params.growth_function();
        for (energy_index, energy) in ENERGY_FLOORS.iter().enumerate() {
            println!(
                "==== {} (μ = {:.4}、σ = {:.4})、エネルギーの下限 {energy} ====",
                animal.code, base.center, base.width
            );
            let baseline = per_trial(animal_index, energy_index, 0);
            let survived_baseline: Vec<&Outcome> = baseline
                .iter()
                .filter(|o| matches!(o.fate, Fate::Survived))
                .collect();
            let pairs = survived_baseline.len();
            // 「そのまま」の試行 i と i+1 の差。表情をかけなくても生じる差の目安
            let noise = |difference: fn(&Features, &Features) -> f32,
                         window: fn(&Outcome) -> &Features| {
                mean((0..pairs).filter(|_| pairs > 1).map(|i| {
                    difference(
                        window(survived_baseline[i]),
                        window(survived_baseline[(i + 1) % pairs]),
                    )
                }))
            };
            println!(
                "  ゆらぎ: 動き差 {} / 形差 {} | 戻した後: 動き差 {} / 形差 {}",
                format_mean(noise(motion_difference, |o| &o.expressed)),
                format_mean(noise(shape_difference, |o| &o.expressed)),
                format_mean(noise(motion_difference, |o| &o.returned)),
                format_mean(noise(shape_difference, |o| &o.returned))
            );
            println!(
                "  {:14} | 崩壊 膨張 | 速さ   脈動  総量   点数   穴   | 動き差 形差 | 戻した後: 動き差 形差 | 表情⇔戻した後: 動き差 形差"
                ,"条件"
            );
            for (expression_index, expression) in EXPRESSIONS.iter().enumerate() {
                let outcomes = per_trial(animal_index, energy_index, expression_index);
                let collapsed: Vec<&str> = outcomes
                    .iter()
                    .filter_map(|o| match o.fate {
                        Fate::Collapsed { phase } => Some(phase),
                        _ => None,
                    })
                    .collect();
                let exploded: Vec<&str> = outcomes
                    .iter()
                    .filter_map(|o| match o.fate {
                        Fate::Exploded { phase } => Some(phase),
                        _ => None,
                    })
                    .collect();
                // 両方が生き延びた同じ試行番号の組だけで比べる
                let paired: Vec<(&Outcome, &Outcome)> = outcomes
                    .iter()
                    .zip(baseline)
                    .filter(|(o, b)| {
                        matches!(o.fate, Fate::Survived) && matches!(b.fate, Fate::Survived)
                    })
                    .collect();
                let survivors: Vec<&Features> = outcomes
                    .iter()
                    .filter(|o| matches!(o.fate, Fate::Survived))
                    .map(|o| &o.expressed)
                    .collect();
                let feature = |get: fn(&Features) -> f32| mean(survivors.iter().map(|f| get(f)));
                println!(
                    "  {:14} | {:>2}/{TRIALS}  {:>2}/{TRIALS} | {:>5} {:>5} {:>6} {:>6} {:>5} | {:>5} {:>5} | {:>5} {:>5} | {:>5} {:>5}{}",
                    expression.label,
                    collapsed.len(),
                    exploded.len(),
                    feature(|f| f.mean_speed).map_or("-".into(), |v| format!("{v:.3}")),
                    format_mean(feature(|f| f.mass_deviation)),
                    feature(|f| f.mean_mass).map_or("-".into(), |v| format!("{v:.1}")),
                    feature(|f| f.visible_area).map_or("-".into(), |v| format!("{v:.1}")),
                    feature(|f| f.hole_cells).map_or("-".into(), |v| format!("{v:.1}")),
                    format_mean(mean(paired.iter().map(|(o, b)| motion_difference(&o.expressed, &b.expressed)))),
                    format_mean(mean(paired.iter().map(|(o, b)| shape_difference(&o.expressed, &b.expressed)))),
                    format_mean(mean(paired.iter().map(|(o, b)| motion_difference(&o.returned, &b.returned)))),
                    format_mean(mean(paired.iter().map(|(o, b)| shape_difference(&o.returned, &b.returned)))),
                    format_mean(mean(paired.iter().map(|(o, _)| motion_difference(&o.expressed, &o.returned)))),
                    format_mean(mean(paired.iter().map(|(o, _)| shape_difference(&o.expressed, &o.returned)))),
                    {
                        let mut phases: Vec<&str> = collapsed.iter().chain(&exploded).copied().collect();
                        phases.dedup();
                        if phases.is_empty() {
                            String::new()
                        } else {
                            format!("  (壊れた区間: {})", phases.join("・"))
                        }
                    }
                );
            }
            println!();
        }
    }
    println!("合計 {:.0} 秒", started.elapsed().as_secs_f32());
}
