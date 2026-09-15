//! 【実験】パラメータが分かっているとき、場の状態も知ると崩壊の確率をよりよく当てられるか。
//!
//! docs/experiments/collapse-prediction.md「崩壊を場から先読みできるか」までの実験は、
//! 広くランダムに負荷をかけて観察し、1回きりの「崩壊した/しなかった」を当てていた。一般的な
//! 予測モデルの作り方に照らして、次のように組み直した最小の実験
//! (docs/experiments/collapse-prediction.md「パラメータと場の両方から崩壊の確率を当てる」)。
//!
//! - **使い道**: 「いまの体の状態で、パラメータ(成長の強さ・テンポ)をここへ動かしたら、
//!   30秒のうちに崩壊するか」。状態に応じて安全な範囲を変えられるかの判断材料
//! - **分岐で作る**: 安全な範囲で履歴の違う出発点を作り、同じ出発点から、場にごく小さな
//!   ゆらぎ(各セル ±0.1%)を掛けて何本も走らせる。カオス系なので崩壊は偶然に左右される。
//!   1回きりの結果ではなく、崩壊した本数の割合を当てる
//! - **境界の近くを調べる**: 成長の強さを崖の近くまで下げる設定と、テンポを安全な範囲の
//!   外まで上げる設定を半分ずつ
//! - **比べる**: パラメータだけ / パラメータと場 / 場だけ。パラメータ側にも2乗と積と、
//!   生物ごとの掛け算を入れ、パラメータの扱いが粗いせいで場が補っているように見えるのを避ける
//! - **評価**: 出発点の単位で5分割の交差検証。二項対数損失とブライアースコア、
//!   「パラメータだけ」との対数損失の差には、出発点の単位のブートストラップで95%区間を
//!   付ける。予測した確率と実際に崩壊した割合の対応(較正)も見る
//!
//! ペット本体の振る舞いには触れず、`Lenia` と `Field` を直接進める。

use std::collections::VecDeque;
use std::thread;
use std::time::Instant;

use vmc_pet_body::appearance::MIN_VISIBLE_VALUE;
use vmc_pet_body::{load_animal, Animal, Field, Lenia};

const FIELD_SIZE: usize = 32;
const CELLS: usize = FIELD_SIZE * FIELD_SIZE;
/// 置いたばかりの生物を落ち着かせるステップ数。後半を健康なときの基準に使う。
const WARMUP_STEPS: usize = 60;
/// `Pet::step` と同じ崩壊の判定。
const COLLAPSE_MASS: f32 = 5.0;
const ONE_SECOND_STEPS: usize = 15;
const TEN_SECONDS_STEPS: usize = 150;

const START_STATES_PER_ANIMAL: usize = 40;
const SETTINGS_PER_STATE: usize = 6;
const REPLICATES: u32 = 8;
/// 目標のパラメータへ移してから崩壊を見る長さ(30秒)。
const HORIZON_STEPS: u32 = 450;
/// 目標のパラメータへ移すのにかける長さ(2秒)。
const RAMP_STEPS: u32 = 30;
/// 分岐ごとに各セルへ掛けるゆらぎの幅。
const NOISE: f32 = 1e-3;
/// 出発点を作るとき、安全な範囲のパラメータを取り替える間隔(10秒)。
const SEGMENT_STEPS: u32 = 150;

const FOLDS: usize = 5;
const BOOTSTRAP_ROUNDS: usize = 1_000;

const SUMMARY_FEATURES: usize = 8;
const FIELD_STATS: usize = 12;
const STATE_FEATURES: usize = SUMMARY_FEATURES + FIELD_STATS;

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

    fn index(&mut self, len: usize) -> usize {
        (self.next_u64() % len as u64) as usize
    }
}

fn seed_for(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash | 1
}

/// 1ステップぶんの、画面から見える量。
#[derive(Clone, Copy)]
struct Measure {
    mass: f32,
    visible_cells: f32,
    peak: f32,
    centroid: Option<(f32, f32)>,
    imbalance: f32,
}

/// トーラス上の符号付き最短距離。
fn wrap(offset: f32, size: f32) -> f32 {
    let offset = offset.rem_euclid(size);
    if offset > size / 2.0 {
        offset - size
    } else {
        offset
    }
}

fn skewness(third_moment: f32, variance: f32) -> f32 {
    if variance <= 1e-6 {
        0.0
    } else {
        third_moment / (variance * variance.sqrt())
    }
}

fn measure(field: &Field) -> Measure {
    let view = field.view();
    let (width, height) = (view.width(), view.height());
    let centroid = view.toroidal_centroid();
    let (mut mass, mut cells, mut peak) = (0.0f32, 0.0f32, 0.0f32);
    let (mut var_x, mut var_y, mut m3_x, mut m3_y) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
    for y in 0..height {
        for x in 0..width {
            let value = view.get(x, y);
            if value <= 0.0 {
                continue;
            }
            mass += value;
            peak = peak.max(value);
            if value > MIN_VISIBLE_VALUE {
                cells += 1.0;
            }
            if let Some((cx, cy)) = centroid {
                let dx = wrap(x as f32 - cx, width as f32);
                let dy = wrap(y as f32 - cy, height as f32);
                var_x += value * dx * dx;
                var_y += value * dy * dy;
                m3_x += value * dx * dx * dx;
                m3_y += value * dy * dy * dy;
            }
        }
    }
    let imbalance = if mass > 0.0 {
        let sx = skewness(m3_x / mass, var_x / mass);
        let sy = skewness(m3_y / mass, var_y / mass);
        (sx * sx + sy * sy).sqrt()
    } else {
        0.0
    };
    Measure {
        mass,
        visible_cells: cells,
        peak,
        centroid,
        imbalance,
    }
}

fn snapshot(field: &Field) -> Vec<f32> {
    let view = field.view();
    let mut values = Vec::with_capacity(CELLS);
    for y in 0..view.height() {
        for x in 0..view.width() {
            values.push(view.get(x, y));
        }
    }
    values
}

fn push_bounded<T>(queue: &mut VecDeque<T>, item: T, capacity: usize) {
    queue.push_back(item);
    while queue.len() > capacity {
        queue.pop_front();
    }
}

/// 健康なときの基準。特徴を生物をまたいで比べられるよう、これで割る。
struct Reference {
    mass: f32,
    cells: f32,
    density: f32,
}

impl Reference {
    fn from(measures: &[Measure]) -> Self {
        let count = measures.len() as f32;
        let mass = measures.iter().map(|m| m.mass).sum::<f32>() / count;
        let cells = measures.iter().map(|m| m.visible_cells).sum::<f32>() / count;
        Reference {
            mass,
            cells,
            density: mass / cells.max(1.0),
        }
    }
}

/// 見える量の要約8つ(`predict_collapse` と同じ定義)。
fn summary_features(history: &VecDeque<Measure>, reference: &Reference) -> [f32; SUMMARY_FEATURES] {
    let latest = history.len() - 1;
    let now = history[latest];
    let one_second_ago = history[latest - ONE_SECOND_STEPS];
    let ten_seconds_ago = history[latest - TEN_SECONDS_STEPS];
    let size = FIELD_SIZE as f32;
    let (mut travelled, mut pairs) = (0.0f32, 0u32);
    for index in (latest - ONE_SECOND_STEPS + 1)..=latest {
        if let (Some(a), Some(b)) = (history[index - 1].centroid, history[index].centroid) {
            let dx = wrap(b.0 - a.0, size);
            let dy = wrap(b.1 - a.1, size);
            travelled += (dx * dx + dy * dy).sqrt();
            pairs += 1;
        }
    }
    let density = if now.visible_cells > 0.0 {
        now.mass / now.visible_cells / reference.density
    } else {
        0.0
    };
    [
        now.mass / reference.mass,
        (now.mass - one_second_ago.mass) / reference.mass,
        (now.mass - ten_seconds_ago.mass) / reference.mass,
        now.visible_cells / reference.cells,
        density,
        now.peak,
        if pairs > 0 {
            travelled / pairs as f32
        } else {
            0.0
        },
        now.imbalance,
    ]
}

/// 場の分布の特徴12(`predict_collapse --field` と同じ定義)。
fn field_stats(recent: &VecDeque<Vec<f32>>, reference: &Reference) -> [f32; FIELD_STATS] {
    let latest = recent.len() - 1;
    let now = &recent[latest];
    let previous = &recent[latest - 1];
    let second_ago = &recent[latest - ONE_SECOND_STEPS];
    let mut stats = [0.0f32; FIELD_STATS];
    let (mut mass_now, mut mass_before) = (0.0f32, 0.0f32);
    for ((&value, &before), &long_before) in now.iter().zip(previous).zip(second_ago) {
        mass_now += value;
        mass_before += before;
        if value > MIN_VISIBLE_VALUE {
            let bin = ((value * 8.0) as usize).min(7);
            stats[bin] += 1.0 / reference.cells;
        }
        if value < before {
            stats[8] += value;
        }
        stats[9] += (value - before).abs();
        if value < long_before {
            stats[11] += value;
        }
    }
    let mass = mass_now.max(1e-6);
    stats[8] /= mass;
    stats[9] /= mass;
    stats[10] = (mass_now - mass_before) / mass;
    stats[11] /= mass;
    stats
}

/// 分岐の出発点。
struct StartState {
    values: Vec<f32>,
    /// 出発点での成長の強さとテンポ。
    parameters: (f32, f32),
    features: [f32; STATE_FEATURES],
}

/// 安全な範囲で10秒ごとにパラメータを取り替えながら、300〜1500ステップ進めた状態。
/// 途中で崩壊したら使わない。
fn start_state(animal: &Animal, rng: &mut Rng) -> Option<StartState> {
    let mut lenia = Lenia::new(animal.params.clone());
    let mut field = Field::new(FIELD_SIZE, FIELD_SIZE);
    field.place_centered(&animal.pattern);
    let mut measures = VecDeque::new();
    let mut recent = VecDeque::new();
    let mut warmup = Vec::with_capacity(WARMUP_STEPS);
    for _ in 0..WARMUP_STEPS {
        lenia.step(&mut field, 1.0);
        let now = measure(&field);
        warmup.push(now);
        push_bounded(&mut measures, now, TEN_SECONDS_STEPS + 1);
        push_bounded(&mut recent, snapshot(&field), ONE_SECOND_STEPS + 1);
    }
    let reference = Reference::from(&warmup[WARMUP_STEPS / 2..]);

    let length = rng.range_u32(300, 1_500);
    let (mut growth, mut tempo) = (1.0f32, 1.0f32);
    for step in 0..length {
        if step.is_multiple_of(SEGMENT_STEPS) {
            growth = rng.range_f32(0.85, 1.0);
            tempo = rng.range_f32(0.5, 1.6);
        }
        lenia.step_at_tempo(&mut field, growth, tempo);
        let now = measure(&field);
        if now.mass < COLLAPSE_MASS {
            return None;
        }
        push_bounded(&mut measures, now, TEN_SECONDS_STEPS + 1);
        push_bounded(&mut recent, snapshot(&field), ONE_SECOND_STEPS + 1);
    }

    let mut features = [0.0f32; STATE_FEATURES];
    features[..SUMMARY_FEATURES].copy_from_slice(&summary_features(&measures, &reference));
    features[SUMMARY_FEATURES..].copy_from_slice(&field_stats(&recent, &reference));
    Some(StartState {
        values: snapshot(&field),
        parameters: (growth, tempo),
        features,
    })
}

/// 境界の近くの目標パラメータ。半分は成長の強さを崖の近くまで下げ、半分はテンポを
/// 安全な範囲の外まで上げる。
fn draw_target(rng: &mut Rng) -> (f32, f32) {
    if rng.unit() < 0.5 {
        let growth = rng.range_f32(0.65, 0.92);
        let tempo = rng.range_f32(0.8, 1.6);
        (growth, tempo)
    } else {
        let growth = rng.range_f32(0.85, 1.0);
        let tempo = rng.range_f32(1.6, 3.5);
        (growth, tempo)
    }
}

/// 出発点にゆらぎを掛けて、2秒かけて目標のパラメータへ移し、30秒のうちに崩壊するか。
fn branch_collapses(
    animal: &Animal,
    start: &StartState,
    target: (f32, f32),
    noise: &[f32],
) -> bool {
    let mut lenia = Lenia::new(animal.params.clone());
    let mut field = Field::new(FIELD_SIZE, FIELD_SIZE);
    field.map(|x, y, _| {
        let index = y * FIELD_SIZE + x;
        (start.values[index] * noise[index]).min(1.0)
    });
    let (from_growth, from_tempo) = start.parameters;
    for step in 0..HORIZON_STEPS {
        let progress = ((step + 1) as f32 / RAMP_STEPS as f32).min(1.0);
        let growth = from_growth + (target.0 - from_growth) * progress;
        let tempo = from_tempo + (target.1 - from_tempo) * progress;
        lenia.step_at_tempo(&mut field, growth, tempo);
        if field.mass() < COLLAPSE_MASS {
            return true;
        }
    }
    false
}

/// 出発点と目標パラメータの1組と、その分岐で崩壊した本数。
struct Group {
    animal: usize,
    state_id: usize,
    state: [f32; STATE_FEATURES],
    from: (f32, f32),
    to: (f32, f32),
    collapsed: u32,
}

fn groups_for(animal_index: usize, code: &str) -> Vec<Group> {
    let animal = load_animal(code).unwrap();
    let mut rng = Rng(seed_for(code));
    let mut groups = Vec::new();
    for state in 0..START_STATES_PER_ANIMAL {
        let state_id = animal_index * START_STATES_PER_ANIMAL + state;
        let Some(start) = start_state(&animal, &mut rng) else {
            continue;
        };
        for _ in 0..SETTINGS_PER_STATE {
            let target = draw_target(&mut rng);
            let mut collapsed = 0;
            for _ in 0..REPLICATES {
                let noise: Vec<f32> = (0..CELLS)
                    .map(|_| 1.0 + rng.range_f32(-NOISE, NOISE))
                    .collect();
                if branch_collapses(&animal, &start, target, &noise) {
                    collapsed += 1;
                }
            }
            groups.push(Group {
                animal: animal_index,
                state_id,
                state: start.features,
                from: start.parameters,
                to: target,
                collapsed,
            });
        }
    }
    groups
}

/// モデルに渡す入力の組。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Inputs {
    Parameters,
    ParametersAndState,
    State,
}

impl Inputs {
    const ALL: [Inputs; 3] = [
        Inputs::Parameters,
        Inputs::ParametersAndState,
        Inputs::State,
    ];

    fn label(self) -> &'static str {
        match self {
            Inputs::Parameters => "パラメータだけ",
            Inputs::ParametersAndState => "パラメータと場",
            Inputs::State => "場だけ",
        }
    }

    /// `animal_count` が `Some` なら、生物の区別(one-hot)も入れる。
    ///
    /// 生物の区別を足し算の偏りとしてだけ入れると、崖の位置が生物ごとに違うことを
    /// パラメータ側で表せず、場がその穴を埋めて「場が効いた」ように見えてしまう
    /// (最初の実行で S1s に起きた)。そこでパラメータの特徴には生物ごとの掛け算も足す。
    /// 場の特徴には足さない(パラメータ側に有利な、控えめな比べ方にする)。
    fn vector(self, group: &Group, animal_count: Option<usize>) -> Vec<f32> {
        let mut x = Vec::new();
        let one_hot: Vec<f32> = animal_count
            .map(|count| {
                (0..count)
                    .map(|a| if group.animal == a { 1.0 } else { 0.0 })
                    .collect()
            })
            .unwrap_or_default();
        if self != Inputs::State {
            let (growth, tempo) = group.to;
            let (from_growth, from_tempo) = group.from;
            let parameters = [
                growth,
                tempo,
                growth * growth,
                tempo * tempo,
                growth * tempo,
                from_growth,
                from_tempo,
                growth - from_growth,
                tempo - from_tempo,
            ];
            x.extend(parameters);
            for indicator in &one_hot {
                x.extend(parameters.iter().map(|p| p * indicator));
            }
        }
        if self != Inputs::Parameters {
            x.extend(group.state);
        }
        x.extend(one_hot);
        x
    }
}

/// 崩壊した本数の割合に合わせる、二項ロジスティック回帰。
struct Fitted {
    mean: Vec<f32>,
    std: Vec<f32>,
    weights: Vec<f64>,
    bias: f64,
}

impl Fitted {
    fn fit(rows: &[(Vec<f32>, u32)]) -> Self {
        const EPOCHS: usize = 3_000;
        const LEARNING_RATE: f64 = 0.3;
        const L2: f64 = 1e-3;
        let n = rows[0].0.len();
        let count = rows.len() as f32;
        let mut mean = vec![0.0f32; n];
        for (x, _) in rows {
            for (m, v) in mean.iter_mut().zip(x) {
                *m += v / count;
            }
        }
        let mut std = vec![0.0f32; n];
        for (x, _) in rows {
            for ((s, v), m) in std.iter_mut().zip(x).zip(&mean) {
                *s += (v - m).powi(2) / count;
            }
        }
        for s in &mut std {
            *s = s.sqrt().max(1e-6);
        }
        let standardized: Vec<(Vec<f64>, f64)> = rows
            .iter()
            .map(|(x, k)| {
                let z = x
                    .iter()
                    .zip(&mean)
                    .zip(&std)
                    .map(|((v, m), s)| ((v - m) / s) as f64)
                    .collect();
                (z, *k as f64)
            })
            .collect();
        let replicates = REPLICATES as f64;
        let total = rows.len() as f64 * replicates;
        let mut weights = vec![0.0f64; n];
        let mut bias = 0.0f64;
        for _ in 0..EPOCHS {
            let mut gradient = vec![0.0f64; n];
            let mut bias_gradient = 0.0f64;
            for (z, k) in &standardized {
                let logit = bias + z.iter().zip(&weights).map(|(a, w)| a * w).sum::<f64>();
                let p = 1.0 / (1.0 + (-logit).exp());
                let error = replicates * p - k;
                for (g, a) in gradient.iter_mut().zip(z) {
                    *g += error * a;
                }
                bias_gradient += error;
            }
            for (w, g) in weights.iter_mut().zip(&gradient) {
                *w -= LEARNING_RATE * (g / total + L2 * *w);
            }
            bias -= LEARNING_RATE * bias_gradient / total;
        }
        Fitted {
            mean,
            std,
            weights,
            bias,
        }
    }

    fn predict(&self, x: &[f32]) -> f64 {
        let logit = self.bias
            + x.iter()
                .zip(&self.mean)
                .zip(&self.std)
                .zip(&self.weights)
                .map(|(((v, m), s), w)| ((v - m) / s) as f64 * w)
                .sum::<f64>();
        1.0 / (1.0 + (-logit).exp())
    }
}

/// 1組ぶんの二項対数損失の合計(分岐1本あたりに直すのは呼び出し側)。
fn group_log_loss(collapsed: u32, p: f64) -> f64 {
    let p = p.clamp(1e-9, 1.0 - 1e-9);
    let k = collapsed as f64;
    let survived = REPLICATES as f64 - k;
    -(k * p.ln() + survived * (1.0 - p).ln())
}

fn group_brier(collapsed: u32, p: f64) -> f64 {
    let k = collapsed as f64;
    let survived = REPLICATES as f64 - k;
    k * (1.0 - p).powi(2) + survived * p.powi(2)
}

/// 出発点の単位で分けた交差検証で、各組の予測を求める。
fn out_of_fold(groups: &[Group], inputs: Inputs, animal_count: Option<usize>) -> Vec<f64> {
    let mut predictions = vec![f64::NAN; groups.len()];
    for fold in 0..FOLDS {
        let rows: Vec<(Vec<f32>, u32)> = groups
            .iter()
            .filter(|g| g.state_id % FOLDS != fold)
            .map(|g| (inputs.vector(g, animal_count), g.collapsed))
            .collect();
        let model = Fitted::fit(&rows);
        for (prediction, group) in predictions.iter_mut().zip(groups) {
            if group.state_id % FOLDS == fold {
                *prediction = model.predict(&inputs.vector(group, animal_count));
            }
        }
    }
    predictions
}

/// 分岐1本あたりの対数損失とブライアースコア。
fn scores(groups: &[&Group], predictions: &[f64]) -> (f64, f64) {
    let replicates = groups.len() as f64 * REPLICATES as f64;
    let log_loss: f64 = groups
        .iter()
        .zip(predictions)
        .map(|(g, p)| group_log_loss(g.collapsed, *p))
        .sum();
    let brier: f64 = groups
        .iter()
        .zip(predictions)
        .map(|(g, p)| group_brier(g.collapsed, *p))
        .sum();
    (log_loss / replicates, brier / replicates)
}

/// `baseline` の対数損失から `candidate` の対数損失を引いた差(正なら `candidate` が良い)の、
/// 出発点の単位のブートストラップによる平均と95%区間。
fn bootstrap_improvement(
    groups: &[&Group],
    baseline: &[f64],
    candidate: &[f64],
    rng: &mut Rng,
) -> (f64, f64, f64) {
    let mut states: Vec<usize> = groups.iter().map(|g| g.state_id).collect();
    states.sort_unstable();
    states.dedup();
    // 出発点ごとの (損失の差の合計, 分岐の本数)
    let per_state: Vec<(f64, f64)> = states
        .iter()
        .map(|state| {
            groups
                .iter()
                .zip(baseline.iter().zip(candidate))
                .filter(|(g, _)| g.state_id == *state)
                .fold((0.0, 0.0), |(difference, count), (g, (b, c))| {
                    (
                        difference + group_log_loss(g.collapsed, *b)
                            - group_log_loss(g.collapsed, *c),
                        count + REPLICATES as f64,
                    )
                })
        })
        .collect();
    let total: (f64, f64) = per_state
        .iter()
        .fold((0.0, 0.0), |acc, s| (acc.0 + s.0, acc.1 + s.1));
    let mut rounds: Vec<f64> = (0..BOOTSTRAP_ROUNDS)
        .map(|_| {
            let mut sum = (0.0, 0.0);
            for _ in 0..per_state.len() {
                let pick = per_state[rng.index(per_state.len())];
                sum = (sum.0 + pick.0, sum.1 + pick.1);
            }
            sum.0 / sum.1
        })
        .collect();
    rounds.sort_by(|a, b| a.total_cmp(b));
    let at = |q: f64| rounds[((q * (rounds.len() - 1) as f64).round()) as usize];
    (total.0 / total.1, at(0.025), at(0.975))
}

fn print_calibration(label: &str, groups: &[&Group], predictions: &[f64]) {
    const BINS: usize = 5;
    let mut line = format!("  {label:14}");
    for bin in 0..BINS {
        let (low, high) = (bin as f64 / BINS as f64, (bin + 1) as f64 / BINS as f64);
        let members: Vec<(&&Group, &f64)> = groups
            .iter()
            .zip(predictions)
            .filter(|(_, p)| **p >= low && (**p < high || (bin == BINS - 1 && **p <= high)))
            .collect();
        if members.is_empty() {
            line += &format!(" | {low:.1}〜{high:.1}: -");
            continue;
        }
        let predicted = members.iter().map(|(_, p)| **p).sum::<f64>() / members.len() as f64;
        let observed = members.iter().map(|(g, _)| g.collapsed as f64).sum::<f64>()
            / (members.len() as f64 * REPLICATES as f64);
        line += &format!(
            " | {low:.1}〜{high:.1}: {:3}組 予測 {predicted:.2} 実際 {observed:.2}",
            members.len()
        );
    }
    println!("{line}");
}

fn main() {
    let started = Instant::now();
    let animals: Vec<String> = vmc_pet_body::list_animals()
        .unwrap()
        .into_iter()
        .map(|(code, _)| code)
        .collect();
    let groups: Vec<Group> = thread::scope(|scope| {
        let handles: Vec<_> = animals
            .iter()
            .enumerate()
            .map(|(index, code)| scope.spawn(move || groups_for(index, code)))
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().unwrap())
            .collect()
    });
    println!(
        "分岐を作った: {} 組 × {REPLICATES} 本、{:.0} 秒",
        groups.len(),
        started.elapsed().as_secs_f32()
    );
    println!();

    println!("組ごとの崩壊した本数(0〜{REPLICATES})の分布。0 でも {REPLICATES} でもない組ほど、偶然に左右されている");
    for (index, code) in animals.iter().enumerate() {
        let mine: Vec<&Group> = groups.iter().filter(|g| g.animal == index).collect();
        let mut histogram = vec![0usize; REPLICATES as usize + 1];
        for g in &mine {
            histogram[g.collapsed as usize] += 1;
        }
        let mixed = mine
            .iter()
            .filter(|g| g.collapsed > 0 && g.collapsed < REPLICATES)
            .count();
        let states = {
            let mut ids: Vec<usize> = mine.iter().map(|g| g.state_id).collect();
            ids.dedup();
            ids.len()
        };
        println!(
            "  {code:6} 出発点 {states:2}(作る途中で崩壊 {:2}) / 組 {:3} | 本数の分布 {histogram:?} | 割れた組 {mixed}",
            START_STATES_PER_ANIMAL - states,
            mine.len()
        );
    }
    println!();

    let all: Vec<&Group> = groups.iter().collect();
    let animal_count = Some(animals.len());
    let predictions: Vec<(Inputs, Vec<f64>)> = Inputs::ALL
        .iter()
        .map(|inputs| (*inputs, out_of_fold(&groups, *inputs, animal_count)))
        .collect();

    let replicates_total = groups.len() as f64 * REPLICATES as f64;
    let collapse_rate = groups.iter().map(|g| g.collapsed as f64).sum::<f64>() / replicates_total;
    let constant = vec![collapse_rate; groups.len()];
    let saturated: Vec<f64> = groups
        .iter()
        .map(|g| g.collapsed as f64 / REPLICATES as f64)
        .collect();
    println!("==== 出発点の単位の5分割交差検証(生物の区別あり) ====");
    println!("  分岐1本あたりの二項対数損失 / ブライアースコア");
    let (constant_loss, constant_brier) = scores(&all, &constant);
    println!(
        "  {:14} {constant_loss:.4} / {constant_brier:.4}(崩壊率 {collapse_rate:.3} を常に答える)",
        "定数"
    );
    for (inputs, prediction) in &predictions {
        let (loss, brier) = scores(&all, prediction);
        println!("  {:14} {loss:.4} / {brier:.4}", inputs.label());
    }
    let (floor, _) = scores(&all, &saturated);
    println!(
        "  {:14} {floor:.4}(各組の実際の割合をそのまま答えた場合。偶然による下限の目安)",
        "下限の目安"
    );
    println!();

    let mut rng = Rng(seed_for("bootstrap"));
    let parameters = &predictions[0].1;
    println!("  「パラメータだけ」からの対数損失の改善(正なら良くなった。出発点の単位のブートストラップ95%区間)");
    for (inputs, prediction) in predictions.iter().skip(1) {
        let (mean, low, high) = bootstrap_improvement(&all, parameters, prediction, &mut rng);
        println!(
            "  {:14} {mean:+.4}  [{low:+.4}, {high:+.4}]",
            inputs.label()
        );
    }
    let (parameters_loss, _) = scores(&all, parameters);
    let (both_loss, _) = scores(&all, &predictions[1].1);
    println!(
        "  パラメータだけから下限の目安までの差のうち、場を足して縮まった割合: {:.1}%",
        (parameters_loss - both_loss) / (parameters_loss - floor) * 100.0
    );
    println!();

    println!("  較正(予測した確率の区間ごとの、予測の平均と実際に崩壊した割合)");
    for (inputs, prediction) in predictions.iter().take(2) {
        print_calibration(inputs.label(), &all, prediction);
    }
    println!();

    println!("==== 生物ごとの内訳(同じ交差検証の予測を生物ごとに集計) ====");
    for (index, code) in animals.iter().enumerate() {
        let indices: Vec<usize> = (0..groups.len())
            .filter(|i| groups[*i].animal == index)
            .collect();
        let mine: Vec<&Group> = indices.iter().map(|i| &groups[*i]).collect();
        let pick = |p: &[f64]| indices.iter().map(|i| p[*i]).collect::<Vec<f64>>();
        let (p_loss, _) = scores(&mine, &pick(parameters));
        let (b_loss, _) = scores(&mine, &pick(&predictions[1].1));
        let (mean, low, high) =
            bootstrap_improvement(&mine, &pick(parameters), &pick(&predictions[1].1), &mut rng);
        println!(
            "  {code:6} パラメータだけ {p_loss:.4} / パラメータと場 {b_loss:.4} | 改善 {mean:+.4} [{low:+.4}, {high:+.4}]"
        );
    }
    println!();

    println!("==== 見ていない生物(生物の区別は入れない) ====");
    for (index, code) in animals.iter().enumerate() {
        let test: Vec<&Group> = groups.iter().filter(|g| g.animal == index).collect();
        let mut per_input = Vec::new();
        for inputs in [Inputs::Parameters, Inputs::ParametersAndState] {
            let rows: Vec<(Vec<f32>, u32)> = groups
                .iter()
                .filter(|g| g.animal != index)
                .map(|g| (inputs.vector(g, None), g.collapsed))
                .collect();
            let model = Fitted::fit(&rows);
            let prediction: Vec<f64> = test
                .iter()
                .map(|g| model.predict(&inputs.vector(g, None)))
                .collect();
            per_input.push(prediction);
        }
        let (p_loss, _) = scores(&test, &per_input[0]);
        let (b_loss, _) = scores(&test, &per_input[1]);
        let (mean, low, high) =
            bootstrap_improvement(&test, &per_input[0], &per_input[1], &mut rng);
        println!(
            "  {code:6}を見ずに学習: パラメータだけ {p_loss:.4} / パラメータと場 {b_loss:.4} | 改善 {mean:+.4} [{low:+.4}, {high:+.4}]"
        );
    }
    println!();
    println!("合計 {:.0} 秒", started.elapsed().as_secs_f32());
}
