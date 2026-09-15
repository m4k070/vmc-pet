//! 【実験】変形しても死なず、元に戻り、見た目の幅が大きい「柔軟な生物」を探す。
//!
//! 表現に幅が出る体の条件を、ユーザーと3つの性質に分けた
//! (docs/experiments/expression-axes.md「柔軟な生物を探す」): 生存範囲が広いこと、戻れること、
//! 変形が見て分かる大きさであること。「別の形から戻る帰り道はあるか」で形の移り変わりは
//! 一方通行だったので、「元の姿へ戻る」だけを合格にする。
//!
//! **探す範囲**: O2u・OG2g・S1s と同じカーネル(R=13、リング1本、多項式)のまま、成長関数の中心 μ と
//! 幅 σ の格子。各点について、3種のどれかのパターンから出発し、μ・σ を徐々に移して生物を連れていく
//! (ほかの値の生物を一から探すより、既知の生物から地続きにたどる方が見つかりやすいため)。落ち着いた後、
//! 崩壊せず、主な塊が1つで(尾や小さな切れ端は数えない)、場を埋めていないものを候補にする。
//!
//! **試験の一式**(ペットが実際に体に与える範囲の変化。どれも候補の姿の複製から始める):
//!
//! - エネルギー: 成長の強さを 0.85(`MIN_GROWTH_SCALE`)まで弱らせる
//! - テンポ ×0.6(がっかりしきったとき)と ×1.6(`MAX_TEMPO`)
//! - クリック: 実際のクリック(半径 4.5・量 0.2)を重心に5回
//! - 参考: 3倍の量を4セル横に5回。慣れで弱まるペットのクリックでは届かない強さなので、合格の条件と
//!   表現の幅の順位には使わない
//!
//! 変化をかけて保った間の見た目の差(表現の幅)と、戻して保った後の差(戻れたか)を、変化をかけない
//! 候補の姿と比べる。見た目の差は、速さ・脈動・はっきり見える点の数の相対差。止まった体どうしで
//! 相対差が大きく出る癖があったので、分母に下限を置く。
//!
//! 基準として、出荷している4種も自分のパラメータで同じ試験にかける。
//!
//! 使い方: `cargo run --release -p vmc-pet-body --example flexible_search -- <出力先>`。
//! 出力先に、上位の候補の姿を PGM で書き出す。ペット本体の振る舞いには触れない。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread;
use std::time::Instant;

use vmc_pet_body::lenia::GrowthMapping;
use vmc_pet_body::{load_animal, Animal, CellPos, Field, GrowthFunction, Lenia, Perturbation};

const FIELD_SIZE: usize = 32;
const CELLS: usize = FIELD_SIZE * FIELD_SIZE;
const COLLAPSE_MASS: f32 = 5.0;
/// はっきり見える点とみなす値(`persistence_expression.rs` と同じ)。
const CLEARLY_VISIBLE: f32 = 0.2;
const EXPLODED_AREA: usize = CELLS / 2;
/// 塊を数えるときの「生きている」の目安(`glaberish_trial.rs` と同じ)。
const BLOB_THRESHOLD: f32 = 0.1;
/// 主な塊とみなす、塊全体の総量に対する割合。これ未満の塊は、尾や切れ端として数えない。
const MAJOR_BLOB_SHARE: f32 = 0.1;
/// 候補とみなす大きさの上限(はっきり見える点の数)。場の 3 割を超える体は、ペットの場では体と
/// 呼びにくい。
const MAX_CANDIDATE_AREA: f32 = 300.0;

/// 格子。μ は 0.10〜0.34 を 0.02 刻み、σ は 0.010〜0.050 を 0.004 刻み。
const CENTER_STEPS: usize = 13;
const CENTER_START: f32 = 0.10;
const CENTER_STEP: f32 = 0.02;
const WIDTH_STEPS: usize = 11;
const WIDTH_START: f32 = 0.010;
const WIDTH_STEP: f32 = 0.004;
/// 出発する生物(カーネルが同じ3種)。
const STARTS: [&str; 3] = ["O2u", "OG2g", "S1s"];
const REFERENCES: [&str; 4] = ["O2u", "OG2g", "S1s", "2S1v"];

const TRACK_RAMP_STEPS: u32 = 1_500;
const TRACK_SETTLE_STEPS: u32 = 1_500;
const MEASURE_STEPS: u32 = 900;
const TEST_RAMP_STEPS: u32 = 750;
const TEST_HOLD_STEPS: u32 = 1_500;
const RETURN_HOLD_STEPS: u32 = 3_000;
const CLICK_INTERVAL_STEPS: u32 = 15;
const CLICK_RADIUS: f32 = 4.5;
const CLICK_AMOUNT: f32 = 0.20;
const NOISE: f32 = 1e-3;
const TRIALS: u64 = 2;

/// 相対差の分母の下限。速さは滑る S1s(0.356)の約7分の1、脈動は総量 100 前後の 0.5%、
/// 点の数は5セル。これより小さい違いは目で見て分からないとみなす。
const SPEED_FLOOR: f32 = 0.05;
const MASS_DEVIATION_FLOOR: f32 = 0.5;
const AREA_FLOOR: f32 = 5.0;
/// 戻した後の差がこれ未満なら「戻った」。
const RETURNED_DISTANCE: f32 = 0.3;
/// 合格の条件と表現の幅の順位に使う試験の数(`TESTS` の先頭から)。最後の「3倍を横に ×5」は、
/// 慣れで弱まるペットのクリック(量 0.2 が上限)では体に届かない強さなので、参考として出すだけに
/// する。最初は合格の条件に入れていたが、出荷している O2u・2S1v を含む動く体をほぼすべて落とした。
const PASS_TEST_COUNT: usize = 4;
/// 上位として並べる数と、画像を書き出す数。
const TOP_COUNT: usize = 20;
const SNAPSHOT_COUNT: usize = 8;

#[derive(Clone, Copy)]
enum Test {
    Energy { floor: f32 },
    Tempo { tempo: f32 },
    Clicks { amount: f32, offset: i32 },
}

const TESTS: [(&str, Test); 5] = [
    ("弱る 0.85", Test::Energy { floor: 0.85 }),
    ("テンポ ×0.6", Test::Tempo { tempo: 0.6 }),
    ("テンポ ×1.6", Test::Tempo { tempo: 1.6 }),
    (
        "クリック ×5",
        Test::Clicks {
            amount: CLICK_AMOUNT,
            offset: 0,
        },
    ),
    (
        "3倍を横に ×5",
        Test::Clicks {
            amount: 3.0 * CLICK_AMOUNT,
            offset: 4,
        },
    ),
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

/// 60秒ぶんの見た目の特徴。
#[derive(Clone, Copy, Default)]
struct Features {
    mean_speed: f32,
    mass_deviation: f32,
    visible_area: f32,
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

/// 主な塊の数。値が `BLOB_THRESHOLD` を超えるセルがつながった塊(トーラス、上下左右)のうち、
/// 塊全体の総量の `MAJOR_BLOB_SHARE` 以上を持つもの。
///
/// 最初はすべての塊を数えていたが、出荷している O2u と 2S1v が「塊 2」で候補から落ちた。尾の
/// 薄い部分が本体から離れて数えられていたためで、見た目は1匹の体なので、小さな切れ端は数えない。
fn major_blob_count(field: &Field) -> usize {
    let view = field.view();
    let value_at = |index: usize| view.get(index % FIELD_SIZE, index / FIELD_SIZE);
    let mut seen = [false; CELLS];
    let mut blob_masses = Vec::new();
    for start in 0..CELLS {
        if seen[start] || value_at(start) <= BLOB_THRESHOLD {
            continue;
        }
        seen[start] = true;
        let mut stack = vec![start];
        let mut mass = 0.0;
        while let Some(index) = stack.pop() {
            mass += value_at(index);
            let (x, y) = (index % FIELD_SIZE, index / FIELD_SIZE);
            let neighbours = [
                ((x + 1) % FIELD_SIZE, y),
                ((x + FIELD_SIZE - 1) % FIELD_SIZE, y),
                (x, (y + 1) % FIELD_SIZE),
                (x, (y + FIELD_SIZE - 1) % FIELD_SIZE),
            ];
            for (nx, ny) in neighbours {
                let neighbour = ny * FIELD_SIZE + nx;
                if !seen[neighbour] && value_at(neighbour) > BLOB_THRESHOLD {
                    seen[neighbour] = true;
                    stack.push(neighbour);
                }
            }
        }
        blob_masses.push(mass);
    }
    let total: f32 = blob_masses.iter().sum();
    blob_masses
        .iter()
        .filter(|mass| **mass >= total * MAJOR_BLOB_SHARE)
        .count()
}

#[derive(Default)]
struct FeatureRecorder {
    previous_centroid: Option<(f32, f32)>,
    path: f32,
    moves: u32,
    masses: Vec<f32>,
    area_total: f32,
}

impl FeatureRecorder {
    fn record(&mut self, field: &Field) {
        let centroid = field.view().toroidal_centroid();
        if let (Some(a), Some(b)) = (self.previous_centroid, centroid) {
            let (dx, dy) = (toroidal_offset(b.0 - a.0), toroidal_offset(b.1 - a.1));
            self.path += (dx * dx + dy * dy).sqrt();
            self.moves += 1;
        }
        self.previous_centroid = centroid;
        self.masses.push(field.mass());
        self.area_total += visible_area(field) as f32;
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
            visible_area: self.area_total / count,
        }
    }
}

/// 分母に下限を置いた相対差。
fn floored_difference(a: f32, b: f32, floor: f32) -> f32 {
    (a - b).abs() / a.abs().max(b.abs()).max(floor)
}

/// 見た目の差 = 動き差(速さと脈動の相対差の平均) + 形差(はっきり見える点の数の相対差)。
fn distance(a: &Features, b: &Features) -> f32 {
    let motion = (floored_difference(a.mean_speed, b.mean_speed, SPEED_FLOOR)
        + floored_difference(a.mass_deviation, b.mass_deviation, MASS_DEVIATION_FLOOR))
        / 2.0;
    motion + floored_difference(a.visible_area, b.visible_area, AREA_FLOOR)
}

fn snapshot_values(field: &Field) -> Vec<f32> {
    let view = field.view();
    (0..CELLS)
        .map(|index| view.get(index % FIELD_SIZE, index / FIELD_SIZE))
        .collect()
}

fn field_from(values: &[f32]) -> Field {
    let mut field = Field::new(FIELD_SIZE, FIELD_SIZE);
    field.map(|x, y, _| values[y * FIELD_SIZE + x]);
    field
}

fn write_pgm(path: &Path, values: &[f32]) {
    let scale = 4;
    let side = FIELD_SIZE * scale;
    let mut bytes = format!("P5\n{side} {side}\n255\n").into_bytes();
    for y in 0..side {
        for x in 0..side {
            let value = values[(y / scale) * FIELD_SIZE + x / scale].clamp(0.0, 1.0);
            bytes.push((value * 255.0).round() as u8);
        }
    }
    fs::write(path, bytes).expect("PGM を書き出せない");
}

/// 体が壊れたかどうか。
fn broken(field: &Field) -> bool {
    field.mass() < COLLAPSE_MASS || visible_area(field) > EXPLODED_AREA
}

/// 1つの規則(カーネルと成長関数)のもとで体を進める。
struct Body {
    lenia: Lenia,
    field: Field,
    rule: GrowthFunction,
}

impl Body {
    /// 1ステップ進める。壊れたら `false`。
    fn step(&mut self, growth_scale: f32, tempo: f32) -> bool {
        self.lenia
            .step_glaberish(&mut self.field, self.rule, self.rule, growth_scale, tempo);
        !broken(&self.field)
    }

    /// `steps` ステップ保ち、最後の `MEASURE_STEPS` で特徴を測る。壊れたら `None`。
    fn hold(&mut self, steps: u32, growth_scale: f32, tempo: f32) -> Option<Features> {
        let mut recorder = FeatureRecorder::default();
        for step in 0..steps {
            if !self.step(growth_scale, tempo) {
                return None;
            }
            if step >= steps.saturating_sub(MEASURE_STEPS) {
                recorder.record(&self.field);
            }
        }
        Some(recorder.finish())
    }

    fn click(&mut self, amount: f32, offset: i32) {
        let Some((x, y)) = self.field.view().toroidal_centroid() else {
            return;
        };
        let size = FIELD_SIZE as i32;
        self.field.inject(&Perturbation {
            at: CellPos {
                x: (x.round() as i32 + offset).rem_euclid(size) as usize,
                y: (y.round() as i32).rem_euclid(size) as usize,
            },
            radius: CLICK_RADIUS,
            amount,
        });
    }
}

/// 1つの試験の結果。壊れたら `None`。
#[derive(Clone, Copy)]
struct TestResult {
    /// 変化をかけている間の、候補の姿との差。
    expressed: f32,
    /// 戻して保った後の、候補の姿との差。
    returned: f32,
}

/// 候補の姿(`values`)から試験を1つ行う。
fn run_test(
    lenia: Lenia,
    rule: GrowthFunction,
    values: &[f32],
    baseline: &Features,
    test: Test,
) -> Option<TestResult> {
    let mut body = Body {
        lenia,
        field: field_from(values),
        rule,
    };
    let expressed = match test {
        Test::Energy { floor } | Test::Tempo { tempo: floor } => {
            let is_energy = matches!(test, Test::Energy { .. });
            let at = |progress: f32| {
                let value = 1.0 - (1.0 - floor) * progress;
                if is_energy {
                    (value, 1.0)
                } else {
                    (1.0, value)
                }
            };
            for step in 0..TEST_RAMP_STEPS {
                let (growth_scale, tempo) = at(step as f32 / TEST_RAMP_STEPS as f32);
                if !body.step(growth_scale, tempo) {
                    return None;
                }
            }
            let (growth_scale, tempo) = at(1.0);
            let expressed = body.hold(TEST_HOLD_STEPS, growth_scale, tempo)?;
            for step in 0..TEST_RAMP_STEPS {
                let (growth_scale, tempo) = at(1.0 - step as f32 / TEST_RAMP_STEPS as f32);
                if !body.step(growth_scale, tempo) {
                    return None;
                }
            }
            expressed
        }
        Test::Clicks { amount, offset } => {
            // 押された直後の変形を見るので、最初のクリックから MEASURE_STEPS の間を測る
            let mut recorder = FeatureRecorder::default();
            for step in 0..MEASURE_STEPS {
                let clicking =
                    step.is_multiple_of(CLICK_INTERVAL_STEPS) && step / CLICK_INTERVAL_STEPS < 5;
                if clicking {
                    body.click(amount, offset);
                }
                if !body.step(1.0, 1.0) {
                    return None;
                }
                recorder.record(&body.field);
            }
            recorder.finish()
        }
    };
    let returned = body.hold(RETURN_HOLD_STEPS, 1.0, 1.0)?;
    Some(TestResult {
        expressed: distance(&expressed, baseline),
        returned: distance(&returned, baseline),
    })
}

/// 候補1つの評価。
struct Evaluation {
    label: String,
    center: f32,
    width: f32,
    /// 候補になれたか(崩壊・膨張せず、主な塊が1つで、大きすぎない)。
    candidate: bool,
    baseline: Features,
    /// 試験ごと・試行ごとの結果。
    tests: Vec<[Option<TestResult>; TRIALS as usize]>,
    /// 候補の姿(試行0)。
    values: Vec<f32>,
    /// 試行どうしの、候補の姿の差(ゆらぎ)。
    noise: f32,
    /// 候補になれなかった理由(分類, 詳細)。候補なら `None`。
    rejection: Option<(&'static str, String)>,
}

impl Evaluation {
    /// すべての試験・試行で壊れず、戻った。
    fn passes(&self) -> bool {
        self.candidate
            && self.tests.iter().take(PASS_TEST_COUNT).all(|trials| {
                trials
                    .iter()
                    .all(|r| r.is_some_and(|r| r.returned < RETURNED_DISTANCE))
            })
    }

    /// 試験ごとの、試行で平均した表現の幅。壊れた試験は `None`。
    fn expression(&self, test: usize) -> Option<f32> {
        let results: Option<Vec<TestResult>> = self.tests[test].iter().copied().collect();
        results.map(|r| r.iter().map(|r| r.expressed).sum::<f32>() / r.len() as f32)
    }

    fn max_expression(&self) -> f32 {
        (0..PASS_TEST_COUNT)
            .filter_map(|test| self.expression(test))
            .fold(0.0, f32::max)
    }
}

/// `start` のパターンから出発し、成長関数を `target` へ徐々に移して体を連れていき、試験にかける。
fn evaluate(
    label: String,
    start: &Animal,
    kernel_source: &Animal,
    target: GrowthFunction,
) -> Evaluation {
    let base = start.params.growth_function();
    let mut evaluation = Evaluation {
        label,
        center: target.center,
        width: target.width,
        candidate: false,
        baseline: Features::default(),
        tests: Vec::new(),
        values: Vec::new(),
        noise: 0.0,
        rejection: None,
    };
    let mut body = Body {
        lenia: Lenia::new(kernel_source.params.clone()),
        field: Field::new(FIELD_SIZE, FIELD_SIZE),
        rule: base,
    };
    body.field.place_centered(&start.pattern);
    for step in 0..TRACK_RAMP_STEPS {
        let progress = step as f32 / TRACK_RAMP_STEPS as f32;
        body.rule = GrowthFunction {
            mapping: base.mapping,
            center: base.center + (target.center - base.center) * progress,
            width: base.width + (target.width - base.width) * progress,
        };
        if !body.step(1.0, 1.0) {
            evaluation.rejection = Some(("連れていく間に壊れた", format!("{step} ステップ目")));
            return evaluation;
        }
    }
    body.rule = target;
    let settled = snapshot_values(&{
        if body.hold(TRACK_SETTLE_STEPS, 1.0, 1.0).is_none() {
            evaluation.rejection = Some(("落ち着かせる間に壊れた", String::new()));
            return evaluation;
        }
        body.field
    });

    let mut baselines = Vec::new();
    let mut trial_values = Vec::new();
    for trial in 0..TRIALS {
        let mut rng = Rng(seed_for(&evaluation.label, trial));
        let noisy: Vec<f32> = settled
            .iter()
            .map(|value| (value * (1.0 + rng.range_f32(-NOISE, NOISE))).min(1.0))
            .collect();
        let mut trial_body = Body {
            lenia: Lenia::new(kernel_source.params.clone()),
            field: field_from(&noisy),
            rule: target,
        };
        let Some(baseline) = trial_body.hold(MEASURE_STEPS, 1.0, 1.0) else {
            evaluation.rejection = Some(("基準を測る間に壊れた", String::new()));
            return evaluation;
        };
        let blobs = major_blob_count(&trial_body.field);
        if blobs != 1 {
            evaluation.rejection = Some((
                "主な塊が1つでない",
                format!(
                    "塊 {blobs}・点 {:.0}・速さ {:.3}",
                    baseline.visible_area, baseline.mean_speed
                ),
            ));
            return evaluation;
        }
        if baseline.visible_area > MAX_CANDIDATE_AREA {
            evaluation.rejection = Some((
                "大きすぎる",
                format!(
                    "点 {:.0}・速さ {:.3}",
                    baseline.visible_area, baseline.mean_speed
                ),
            ));
            return evaluation;
        }
        baselines.push(baseline);
        trial_values.push(snapshot_values(&trial_body.field));
    }
    evaluation.candidate = true;
    evaluation.baseline = baselines[0];
    evaluation.values = trial_values[0].clone();
    evaluation.noise = distance(&baselines[0], &baselines[1]);
    for (_, test) in TESTS {
        let mut results = [None; TRIALS as usize];
        for trial in 0..TRIALS as usize {
            results[trial] = run_test(
                Lenia::new(kernel_source.params.clone()),
                target,
                &trial_values[trial],
                &baselines[trial],
                test,
            );
        }
        evaluation.tests.push(results);
    }
    evaluation
}

fn format_evaluation(evaluation: &Evaluation) -> String {
    let tests: Vec<String> = (0..TESTS.len())
        .map(|test| {
            let returned_all = evaluation.tests[test]
                .iter()
                .all(|r| r.is_some_and(|r| r.returned < RETURNED_DISTANCE));
            match evaluation.expression(test) {
                None => "  崩壊".to_string(),
                Some(expression) if returned_all => format!("{expression:>6.2}"),
                Some(expression) => format!("{expression:>5.2}✗"),
            }
        })
        .collect();
    format!(
        "{:22} μ={:.3} σ={:.4} | 速さ {:.3} 脈動 {:5.2} 点 {:5.1} | ゆらぎ {:.2} | {} | 最大 {:.2}",
        evaluation.label,
        evaluation.center,
        evaluation.width,
        evaluation.baseline.mean_speed,
        evaluation.baseline.mass_deviation,
        evaluation.baseline.visible_area,
        evaluation.noise,
        tests.join(" "),
        evaluation.max_expression()
    )
}

fn main() {
    let output = PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("出力先のディレクトリを引数で渡す"),
    );
    fs::create_dir_all(&output).expect("出力先を作れない");
    let started = Instant::now();
    let animals: Vec<Animal> = REFERENCES
        .iter()
        .map(|code| load_animal(code).unwrap())
        .collect();
    let find = |code: &str| animals.iter().find(|a| a.code == code).unwrap();

    // (ラベル, 出発する生物, カーネルの元, 目標の成長関数)
    let mut jobs: Vec<(String, &str, &str, GrowthFunction)> = REFERENCES
        .iter()
        .map(|code| {
            (
                format!("基準 {code}"),
                *code,
                *code,
                find(code).params.growth_function(),
            )
        })
        .collect();
    for start in STARTS {
        for center_index in 0..CENTER_STEPS {
            for width_index in 0..WIDTH_STEPS {
                let target = GrowthFunction {
                    mapping: GrowthMapping::Polynomial,
                    center: CENTER_START + CENTER_STEP * center_index as f32,
                    width: WIDTH_START + WIDTH_STEP * width_index as f32,
                };
                jobs.push((
                    format!("{start} から μ{center_index} σ{width_index}"),
                    start,
                    start,
                    target,
                ));
            }
        }
    }
    let job_count = jobs.len();
    let queue = Mutex::new(jobs.into_iter().enumerate());
    let results: Mutex<Vec<Option<Evaluation>>> =
        Mutex::new((0..job_count).map(|_| None).collect());
    let workers = thread::available_parallelism().map_or(4, |n| n.get());
    thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let Some((index, (label, start, kernel, target))) = queue.lock().unwrap().next()
                else {
                    break;
                };
                let evaluation = evaluate(label, find(start), find(kernel), target);
                results.lock().unwrap()[index] = Some(evaluation);
            });
        }
    });
    let results: Vec<Evaluation> = results
        .into_inner()
        .unwrap()
        .into_iter()
        .map(Option::unwrap)
        .collect();

    let test_names: Vec<&str> = TESTS.iter().map(|(name, _)| *name).collect();
    println!(
        "場 {FIELD_SIZE}×{FIELD_SIZE}、各 {TRIALS} 回。数字は変化をかけている間の見た目の差(試行の平均)、✗ は戻らなかった試行がある、崩壊は壊れた試行がある"
    );
    println!(
        "試験: {}(最後の試験は参考。合格の条件と「最大」には、先頭の {PASS_TEST_COUNT} つだけを使う)",
        test_names.join(" / ")
    );
    println!();
    println!("==== 基準(出荷している生物) ====");
    for evaluation in results.iter().filter(|e| e.label.starts_with("基準")) {
        if evaluation.candidate {
            println!(
                "  {}{}",
                format_evaluation(evaluation),
                if evaluation.passes() { "  合格" } else { "" }
            );
        } else {
            let (category, detail) = evaluation
                .rejection
                .clone()
                .unwrap_or(("不明", String::new()));
            println!(
                "  {:22} 候補になれなかった: {category} {detail}",
                evaluation.label
            );
        }
    }

    let searched: Vec<&Evaluation> = results
        .iter()
        .filter(|e| !e.label.starts_with("基準"))
        .collect();
    let candidates: Vec<&&Evaluation> = searched.iter().filter(|e| e.candidate).collect();
    let mut passing: Vec<&&Evaluation> =
        candidates.iter().copied().filter(|e| e.passes()).collect();
    passing.sort_by(|a, b| b.max_expression().total_cmp(&a.max_expression()));
    println!();
    println!(
        "==== 探索: {} 点のうち候補 {}、全試験で壊れず戻った(合格) {} ====",
        searched.len(),
        candidates.len(),
        passing.len()
    );
    let mut categories: Vec<&str> = searched
        .iter()
        .filter_map(|e| e.rejection.as_ref().map(|(category, _)| *category))
        .collect();
    categories.sort_unstable();
    categories.dedup();
    for category in categories {
        let count = searched
            .iter()
            .filter(|e| e.rejection.as_ref().is_some_and(|(c, _)| *c == category))
            .count();
        println!("  候補になれなかった理由: {category} {count}");
    }
    for (rank, evaluation) in passing.iter().take(TOP_COUNT).enumerate() {
        println!("  {:>2}. {}", rank + 1, format_evaluation(evaluation));
        if rank < SNAPSHOT_COUNT {
            let name = format!(
                "top{:02}_{}.pgm",
                rank + 1,
                evaluation.label.replace(' ', "_")
            );
            write_pgm(&output.join(name), &evaluation.values);
        }
    }

    println!();
    println!("==== 候補のうち動いているもの(速さ > 0.1。合格かどうかによらない) ====");
    for evaluation in candidates.iter().filter(|e| e.baseline.mean_speed > 0.1) {
        let name = format!("moving_{}.pgm", evaluation.label.replace(' ', "_"));
        write_pgm(&output.join(name), &evaluation.values);
        println!(
            "  {}{}",
            format_evaluation(evaluation),
            if evaluation.passes() { "  合格" } else { "" }
        );
    }
    println!();
    println!("==== 主な塊が1つでない・大きすぎるもののうち動いているもの(速さ > 0.1) ====");
    for evaluation in searched.iter().filter(|e| {
        e.rejection.as_ref().is_some_and(|(category, detail)| {
            (*category == "主な塊が1つでない" || *category == "大きすぎる")
                && detail
                    .rsplit("速さ ")
                    .next()
                    .and_then(|speed| speed.parse::<f32>().ok())
                    .is_some_and(|speed| speed > 0.1)
        })
    }) {
        let (category, detail) = evaluation.rejection.clone().unwrap();
        println!(
            "  {:22} μ={:.3} σ={:.4} | {category} {detail}",
            evaluation.label, evaluation.center, evaluation.width
        );
    }
    println!();
    println!("==== 試験ごとの、候補のうち壊れた/戻らなかった数 ====");
    for (test, name) in test_names.iter().enumerate() {
        let collapsed = candidates
            .iter()
            .filter(|e| e.tests[test].iter().any(|r| r.is_none()))
            .count();
        let not_returned = candidates
            .iter()
            .filter(|e| {
                e.tests[test].iter().all(|r| r.is_some())
                    && e.tests[test]
                        .iter()
                        .any(|r| r.is_some_and(|r| r.returned >= RETURNED_DISTANCE))
            })
            .count();
        println!("  {name:14} 壊れた {collapsed:>3} / 戻らなかった {not_returned:>3}");
    }
    println!();
    println!("合計 {:.0} 秒", started.elapsed().as_secs_f32());
}
