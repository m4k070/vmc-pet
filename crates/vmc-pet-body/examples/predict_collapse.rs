//! 【実験】体の崩壊を、画面から見える量だけを使う小さなモデルで先読みできるか。
//!
//! いまの安全装置(`Pet::step`)は、総量が 5 を下回った**後**に置き直す。崩壊の前に
//! 気づけるなら、テンポを落として立て直す、といった振る舞いの判断材料になる
//! (World Models の M にあたる「体の先を読む」ものの最初の一歩)。
//!
//! 普段の動かし方ではほとんど崩壊しないので、体に負荷をかけて崩壊する・しない
//! エピソードを作る。`LeniaBody` の安全な範囲(成長の強さ 0.85〜1.0、テンポ ×0.5〜×1.6)を
//! 通さず、`Lenia` と `Field` を直接進める。ペット本体の振る舞いには一切触れない。
//!
//! - 負荷: 飢え(成長の強さを崖の下まで徐々に下げる)・つつき(強い摂動を繰り返す)・
//!   テンポ(徐々に速くする)。負荷なしの健康なエピソードも混ぜる
//! - 特徴: 画面から見える量だけ(総量・その1秒/10秒の変化・広がり・密度・一番濃い値・
//!   速さ・形の偏り)。生物ごとの健康なときの値で割って、生物をまたいで比べられるようにする。
//!   負荷のパラメータそのものはモデルに渡さない
//! - モデル: ロジスティック回帰(重みが読める)。「N ステップ以内に崩壊するか」を当てる
//! - 評価: 学習用の健康な(崩壊しなかった)エピソードで一度も警告が出ないしきい値を決め、
//!   それで評価用のエピソードに誤警報が出ないか、崩壊の何秒前に警告できるかを見る。
//!   「総量が少ない」「総量が減っている」だけの素朴なしきい値と比べ、モデルが上回らなければ
//!   モデルは要らない。見ていない負荷・見ていない生物にも効くかを確かめる

use std::collections::VecDeque;
use std::thread;
use std::time::Instant;

use vmc_pet_body::appearance::MIN_VISIBLE_VALUE;
use vmc_pet_body::{load_animal, CellPos, Field, Lenia, Perturbation};

const FIELD_SIZE: usize = 32;
/// 置いたばかりの生物を落ち着かせるステップ数。後半を健康なときの基準に使う。
const WARMUP_STEPS: usize = 60;
const MAX_EPISODE_STEPS: u32 = 3_000;
/// `Pet::step` と同じ崩壊の判定。
const COLLAPSE_MASS: f32 = 5.0;
/// 何ステップごとに特徴を取り出すか。
const SAMPLE_EVERY: u32 = 5;
/// 1秒(15ステップ)と10秒(150ステップ)。特徴の変化を見る幅。
const ONE_SECOND_STEPS: usize = 15;
const TEN_SECONDS_STEPS: usize = 150;
const STEPS_PER_SECOND: f32 = 15.0;

const CONTROL_EPISODES: u32 = 10;
const EPISODES_PER_STRESSOR: u32 = 30;

const FEATURES: usize = 8;
const FEATURE_NAMES: [&str; FEATURES] = [
    "総量",
    "総量の変化(1秒)",
    "総量の変化(10秒)",
    "広がり",
    "密度",
    "一番濃い値",
    "速さ",
    "形の偏り",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stressor {
    None,
    Starvation,
    Poking,
    Tempo,
}

impl Stressor {
    const STRESSES: [Stressor; 3] = [Stressor::Starvation, Stressor::Poking, Stressor::Tempo];

    fn label(self) -> &'static str {
        match self {
            Stressor::None => "負荷なし",
            Stressor::Starvation => "飢え",
            Stressor::Poking => "つつき",
            Stressor::Tempo => "テンポ",
        }
    }
}

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

/// エピソードごとに決まる負荷のかけ方。
struct Plan {
    stressor: Stressor,
    target_growth: f32,
    target_tempo: f32,
    ramp_steps: u32,
    poke_every: u32,
    poke_amount: f32,
    poke_radius: f32,
}

impl Plan {
    fn draw(stressor: Stressor, rng: &mut Rng) -> Self {
        let mut plan = Plan {
            stressor,
            target_growth: 1.0,
            target_tempo: 1.0,
            ramp_steps: 1,
            poke_every: u32::MAX,
            poke_amount: 0.0,
            poke_radius: 0.0,
        };
        match stressor {
            Stressor::None => {}
            Stressor::Starvation => {
                plan.target_growth = rng.range_f32(0.60, 0.92);
                plan.ramp_steps = rng.range_u32(150, 1_500);
            }
            Stressor::Tempo => {
                plan.target_tempo = rng.range_f32(1.0, 3.5);
                plan.ramp_steps = rng.range_u32(150, 1_500);
            }
            Stressor::Poking => {
                plan.poke_every = rng.range_u32(4, 60);
                plan.poke_amount = rng.range_f32(0.05, 0.6);
                plan.poke_radius = rng.range_f32(2.0, 6.0);
            }
        }
        plan
    }

    /// そのステップの成長の強さとテンポ(負荷を徐々にかけて、目標に着いたら保つ)。
    fn growth_and_tempo(&self, step: u32) -> (f32, f32) {
        let progress = (step as f32 / self.ramp_steps as f32).min(1.0);
        (
            1.0 + (self.target_growth - 1.0) * progress,
            1.0 + (self.target_tempo - 1.0) * progress,
        )
    }

    /// そのステップにつつくなら、体の近く(重心から ±8 セル)への摂動。
    fn poke(&self, step: u32, centroid: Option<(f32, f32)>, rng: &mut Rng) -> Option<Perturbation> {
        if self.stressor != Stressor::Poking || !step.is_multiple_of(self.poke_every) {
            return None;
        }
        let (cx, cy) = centroid.unwrap_or((FIELD_SIZE as f32 / 2.0, FIELD_SIZE as f32 / 2.0));
        let size = FIELD_SIZE as f32;
        let x = (cx + rng.range_f32(-8.0, 8.0)).rem_euclid(size);
        let y = (cy + rng.range_f32(-8.0, 8.0)).rem_euclid(size);
        Some(Perturbation {
            at: CellPos {
                x: (x as usize).min(FIELD_SIZE - 1),
                y: (y as usize).min(FIELD_SIZE - 1),
            },
            radius: self.poke_radius,
            amount: self.poke_amount,
        })
    }
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

fn features_at(history: &VecDeque<Measure>, reference: &Reference) -> [f32; FEATURES] {
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

struct Sample {
    step: u32,
    features: [f32; FEATURES],
}

struct Episode {
    animal: String,
    stressor: Stressor,
    index: u32,
    samples: Vec<Sample>,
    collapse_step: Option<u32>,
}

fn seed_for(code: &str, stressor: Stressor, index: u32) -> u64 {
    // FNV-1a で生物コード・負荷・番号を混ぜる(0 は xorshift で使えないので避ける)
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let bytes = code
        .bytes()
        .chain([stressor as u8])
        .chain(index.to_le_bytes());
    for byte in bytes {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash | 1
}

fn run_episode(code: &str, stressor: Stressor, index: u32) -> Episode {
    let animal = load_animal(code).unwrap();
    let mut lenia = Lenia::new(animal.params.clone());
    let mut field = Field::new(FIELD_SIZE, FIELD_SIZE);
    field.place_centered(&animal.pattern);
    let mut rng = Rng(seed_for(code, stressor, index));

    let mut warmup = Vec::with_capacity(WARMUP_STEPS);
    for _ in 0..WARMUP_STEPS {
        lenia.step(&mut field, 1.0);
        warmup.push(measure(&field));
    }
    let reference = Reference::from(&warmup[WARMUP_STEPS / 2..]);
    let plan = Plan::draw(stressor, &mut rng);

    let mut history: VecDeque<Measure> = warmup.into_iter().collect();
    let mut samples = Vec::new();
    let mut collapse_step = None;
    for step in 0..MAX_EPISODE_STEPS {
        let centroid = history.back().and_then(|m| m.centroid);
        if let Some(perturbation) = plan.poke(step, centroid, &mut rng) {
            field.inject(&perturbation);
        }
        let (growth, tempo) = plan.growth_and_tempo(step);
        lenia.step_at_tempo(&mut field, growth, tempo);
        let now = measure(&field);
        if now.mass < COLLAPSE_MASS {
            collapse_step = Some(step);
            break;
        }
        history.push_back(now);
        while history.len() > TEN_SECONDS_STEPS + 1 {
            history.pop_front();
        }
        if step.is_multiple_of(SAMPLE_EVERY) && history.len() > TEN_SECONDS_STEPS {
            samples.push(Sample {
                step,
                features: features_at(&history, &reference),
            });
        }
    }
    Episode {
        animal: code.to_string(),
        stressor,
        index,
        samples,
        collapse_step,
    }
}

fn episodes_for(code: &str) -> Vec<Episode> {
    let mut episodes = Vec::new();
    for index in 0..CONTROL_EPISODES {
        episodes.push(run_episode(code, Stressor::None, index));
    }
    for stressor in Stressor::STRESSES {
        for index in 0..EPISODES_PER_STRESSOR {
            episodes.push(run_episode(code, stressor, index));
        }
    }
    episodes
}

/// 標準化してからのロジスティック回帰。
struct Model {
    mean: [f32; FEATURES],
    std: [f32; FEATURES],
    weights: [f32; FEATURES],
    bias: f32,
}

impl Model {
    fn standardized(&self, x: &[f32; FEATURES]) -> [f32; FEATURES] {
        let mut z = [0.0; FEATURES];
        for i in 0..FEATURES {
            z[i] = (x[i] - self.mean[i]) / self.std[i];
        }
        z
    }

    /// 崩壊が近いほど大きい(ロジット)。
    fn score(&self, x: &[f32; FEATURES]) -> f32 {
        let z = self.standardized(x);
        self.bias
            + z.iter()
                .zip(self.weights.iter())
                .map(|(a, w)| a * w)
                .sum::<f32>()
    }

    /// 崩壊が近い例と遠い例を同じ重さで扱うよう、少ない側を重くして学習する。
    fn train(data: &[(&[f32; FEATURES], bool)]) -> Option<Model> {
        const EPOCHS: usize = 400;
        const LEARNING_RATE: f64 = 0.5;
        const L2: f64 = 1e-4;
        let positives = data.iter().filter(|(_, y)| *y).count();
        if positives == 0 || positives == data.len() {
            return None;
        }
        let count = data.len() as f32;
        let mut mean = [0.0f32; FEATURES];
        let mut std = [0.0f32; FEATURES];
        for (x, _) in data {
            for i in 0..FEATURES {
                mean[i] += x[i] / count;
            }
        }
        for (x, _) in data {
            for i in 0..FEATURES {
                std[i] += (x[i] - mean[i]).powi(2) / count;
            }
        }
        for s in std.iter_mut() {
            *s = s.sqrt().max(1e-6);
        }
        let mut model = Model {
            mean,
            std,
            weights: [0.0; FEATURES],
            bias: 0.0,
        };
        let standardized: Vec<([f32; FEATURES], bool)> = data
            .iter()
            .map(|(x, y)| (model.standardized(x), *y))
            .collect();
        let positive_weight = (data.len() - positives) as f64 / positives as f64;
        for _ in 0..EPOCHS {
            let mut gradient = [0.0f64; FEATURES];
            let mut bias_gradient = 0.0f64;
            let mut total_weight = 0.0f64;
            for (z, y) in &standardized {
                let logit = model.bias as f64
                    + z.iter()
                        .zip(model.weights.iter())
                        .map(|(a, w)| (*a as f64) * (*w as f64))
                        .sum::<f64>();
                let predicted = 1.0 / (1.0 + (-logit).exp());
                let (target, weight) = if *y {
                    (1.0, positive_weight)
                } else {
                    (0.0, 1.0)
                };
                let error = (predicted - target) * weight;
                for i in 0..FEATURES {
                    gradient[i] += error * z[i] as f64;
                }
                bias_gradient += error;
                total_weight += weight;
            }
            for (weight, gradient) in model.weights.iter_mut().zip(gradient.iter()) {
                let step = gradient / total_weight + L2 * *weight as f64;
                *weight -= (LEARNING_RATE * step) as f32;
            }
            model.bias -= (LEARNING_RATE * bias_gradient / total_weight) as f32;
        }
        Some(model)
    }
}

/// 崩壊の近さを表す点数の付け方。モデルと、素朴なしきい値の比較対象。
enum Scorer<'a> {
    Model(&'a Model),
    LowMass,
    DropOverOneSecond,
    DropOverTenSeconds,
}

impl Scorer<'_> {
    fn label(&self) -> &'static str {
        match self {
            Scorer::Model(_) => "モデル",
            Scorer::LowMass => "総量が少ない",
            Scorer::DropOverOneSecond => "1秒で減った",
            Scorer::DropOverTenSeconds => "10秒で減った",
        }
    }

    fn score(&self, x: &[f32; FEATURES]) -> f32 {
        match self {
            Scorer::Model(model) => model.score(x),
            Scorer::LowMass => -x[0],
            Scorer::DropOverOneSecond => -x[1],
            Scorer::DropOverTenSeconds => -x[2],
        }
    }
}

fn collapses_within(episode: &Episode, sample: &Sample, horizon: u32) -> bool {
    episode
        .collapse_step
        .is_some_and(|collapse| collapse - sample.step <= horizon)
}

/// 順位による AUC(1.0 なら、崩壊が近い例がいつも遠い例より高い点数)。
fn auc(scored: &mut [(f32, bool)]) -> f32 {
    scored.sort_by(|a, b| a.0.total_cmp(&b.0));
    let positives = scored.iter().filter(|(_, y)| *y).count() as f64;
    let negatives = scored.len() as f64 - positives;
    if positives == 0.0 || negatives == 0.0 {
        return f32::NAN;
    }
    let mut rank_sum = 0.0f64;
    let mut start = 0;
    while start < scored.len() {
        let mut end = start;
        while end + 1 < scored.len() && scored[end + 1].0 == scored[start].0 {
            end += 1;
        }
        let average_rank = (start + end) as f64 / 2.0 + 1.0;
        rank_sum += scored[start..=end].iter().filter(|(_, y)| *y).count() as f64 * average_rank;
        start = end + 1;
    }
    ((rank_sum - positives * (positives + 1.0) / 2.0) / (positives * negatives)) as f32
}

struct Report {
    auc: f32,
    false_alarms: usize,
    healthy_episodes: usize,
    detected: usize,
    collapses: usize,
    leads: Vec<u32>,
}

/// 学習用の崩壊しなかったエピソードのうち、`healthy_quantile` の割合で一度も警告が
/// 出ない高さのしきい値。1.0 なら、どの崩壊しなかったエピソードでも一度も警告が出ない。
fn threshold_for(scorer: &Scorer, train: &[&Episode], healthy_quantile: f32) -> f32 {
    let mut episode_peaks: Vec<f32> = train
        .iter()
        .filter(|e| e.collapse_step.is_none())
        .map(|e| {
            e.samples
                .iter()
                .map(|s| scorer.score(&s.features))
                .fold(f32::NEG_INFINITY, f32::max)
        })
        .collect();
    episode_peaks.sort_by(|a, b| a.total_cmp(b));
    let rank = ((healthy_quantile * episode_peaks.len() as f32).ceil() as usize).max(1);
    episode_peaks[rank.min(episode_peaks.len()) - 1]
}

fn evaluate(
    scorer: &Scorer,
    train: &[&Episode],
    test: &[&Episode],
    horizon: u32,
    healthy_quantile: f32,
) -> Report {
    let threshold = threshold_for(scorer, train, healthy_quantile);

    let mut scored: Vec<(f32, bool)> = test
        .iter()
        .flat_map(|e| {
            e.samples
                .iter()
                .map(move |s| (scorer.score(&s.features), collapses_within(e, s, horizon)))
        })
        .collect();
    let auc = auc(&mut scored);

    let mut report = Report {
        auc,
        false_alarms: 0,
        healthy_episodes: 0,
        detected: 0,
        collapses: 0,
        leads: Vec::new(),
    };
    for episode in test {
        let first_warning = episode
            .samples
            .iter()
            .find(|s| scorer.score(&s.features) > threshold);
        match episode.collapse_step {
            None => {
                report.healthy_episodes += 1;
                if first_warning.is_some() {
                    report.false_alarms += 1;
                }
            }
            Some(collapse) => {
                report.collapses += 1;
                if let Some(warning) = first_warning {
                    report.detected += 1;
                    report.leads.push(collapse - warning.step);
                }
            }
        }
    }
    report
}

fn print_reports(title: &str, train: &[&Episode], test: &[&Episode], horizon: u32) {
    print_reports_at(title, train, test, horizon, 1.0);
}

fn print_reports_at(
    title: &str,
    train: &[&Episode],
    test: &[&Episode],
    horizon: u32,
    healthy_quantile: f32,
) {
    let data: Vec<(&[f32; FEATURES], bool)> = train
        .iter()
        .flat_map(|e| {
            e.samples
                .iter()
                .map(move |s| (&s.features, collapses_within(e, s, horizon)))
        })
        .collect();
    let Some(model) = Model::train(&data) else {
        println!("{title}: 学習用のデータに崩壊が近い例が無い(または全部そう)ので学習できない");
        return;
    };
    println!(
        "{title}(学習 {} / 評価 {} エピソード)",
        train.len(),
        test.len()
    );
    let scorers = [
        Scorer::Model(&model),
        Scorer::LowMass,
        Scorer::DropOverOneSecond,
        Scorer::DropOverTenSeconds,
    ];
    for scorer in &scorers {
        let report = evaluate(scorer, train, test, horizon, healthy_quantile);
        let mut leads = report.leads.clone();
        leads.sort_unstable();
        let median = leads.get(leads.len() / 2).map_or("  -".to_string(), |l| {
            format!("{:5.1}", *l as f32 / STEPS_PER_SECOND)
        });
        let at_least = |seconds: f32| {
            leads
                .iter()
                .filter(|l| **l as f32 >= seconds * STEPS_PER_SECOND)
                .count()
        };
        println!(
            "  {:10} AUC {:.3} | 誤警報 {:2}/{:2} | 先読み {:3}/{:3}(1秒以上前 {:3}、10秒以上前 {:3})| 何秒前(中央値) {median}",
            scorer.label(),
            report.auc,
            report.false_alarms,
            report.healthy_episodes,
            report.detected,
            report.collapses,
            at_least(1.0),
            at_least(10.0),
        );
    }
    println!();
}

fn main() {
    let started = Instant::now();
    let animals: Vec<String> = vmc_pet_body::list_animals()
        .unwrap()
        .into_iter()
        .map(|(code, _)| code)
        .collect();
    let episodes: Vec<Episode> = thread::scope(|scope| {
        let handles: Vec<_> = animals
            .iter()
            .map(|code| scope.spawn(move || episodes_for(code)))
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().unwrap())
            .collect()
    });
    println!(
        "エピソードを作った: {} 本、{:.0} 秒",
        episodes.len(),
        started.elapsed().as_secs_f32()
    );
    println!();

    println!("崩壊した数(崩壊までの中央値ステップ)");
    for code in &animals {
        let mut line = format!("  {code:6}");
        for stressor in [
            Stressor::None,
            Stressor::Starvation,
            Stressor::Poking,
            Stressor::Tempo,
        ] {
            let mut steps: Vec<u32> = episodes
                .iter()
                .filter(|e| &e.animal == code && e.stressor == stressor)
                .filter_map(|e| e.collapse_step)
                .collect();
            let total = episodes
                .iter()
                .filter(|e| &e.animal == code && e.stressor == stressor)
                .count();
            steps.sort_unstable();
            let median = steps
                .get(steps.len() / 2)
                .map_or("-".to_string(), |s| s.to_string());
            line += &format!(
                " | {} {:2}/{:2}({median:>4})",
                stressor.label(),
                steps.len(),
                total
            );
        }
        println!("{line}");
    }
    println!();

    let all: Vec<&Episode> = episodes.iter().collect();
    let train: Vec<&Episode> = all.iter().copied().filter(|e| e.index % 2 == 0).collect();
    let test: Vec<&Episode> = all.iter().copied().filter(|e| e.index % 2 == 1).collect();

    for horizon in [150u32, 450] {
        println!(
            "==== {:.0}秒以内({horizon}ステップ)に崩壊するか ====",
            horizon as f32 / STEPS_PER_SECOND
        );
        print_reports("同じ負荷・同じ生物の別エピソード", &train, &test, horizon);
    }

    let horizon = 150;
    let data: Vec<(&[f32; FEATURES], bool)> = train
        .iter()
        .flat_map(|e| {
            e.samples
                .iter()
                .map(move |s| (&s.features, collapses_within(e, s, horizon)))
        })
        .collect();
    if let Some(model) = Model::train(&data) {
        println!("学習した重み(標準化した特徴に対する。正なら崩壊が近いほど大きい)");
        for (name, weight) in FEATURE_NAMES.iter().zip(model.weights.iter()) {
            println!("  {name:14} {weight:+.3}");
        }
        println!();
    }

    println!("==== 見ていない負荷(10秒以内) ====");
    for held_out in Stressor::STRESSES {
        let train: Vec<&Episode> = all
            .iter()
            .copied()
            .filter(|e| e.stressor != held_out)
            .collect();
        let test: Vec<&Episode> = all
            .iter()
            .copied()
            .filter(|e| e.stressor == held_out)
            .collect();
        print_reports(
            &format!("{}を見ずに学習", held_out.label()),
            &train,
            &test,
            horizon,
        );
    }

    println!("==== 見ていない生物(10秒以内) ====");
    for held_out in &animals {
        let train: Vec<&Episode> = all
            .iter()
            .copied()
            .filter(|e| &e.animal != held_out)
            .collect();
        let test: Vec<&Episode> = all
            .iter()
            .copied()
            .filter(|e| &e.animal == held_out)
            .collect();
        print_reports(&format!("{held_out}を見ずに学習"), &train, &test, horizon);
    }
    println!("==== しきい値を緩めたとき(10秒以内、同じ負荷・同じ生物の別エピソード) ====");
    for quantile in [0.95f32, 0.90, 0.80] {
        print_reports_at(
            &format!(
                "学習用の崩壊しなかったエピソードの {:.0}% で警告が出ない高さ",
                quantile * 100.0
            ),
            &train,
            &test,
            horizon,
            quantile,
        );
    }

    print_precursors(&episodes);
    println!("合計 {:.0} 秒", started.elapsed().as_secs_f32());
}

/// 崩壊の前に見える量がどう変わるかと、崩壊しなかったエピソードがどこまで下がるかを並べる。
/// 崩壊の手前の値が、生き延びたエピソードの底と重なっていれば、見える量では区別できない。
fn print_precursors(episodes: &[Episode]) {
    const BEFORE_STEPS: [u32; 7] = [450, 150, 75, 30, 15, 10, 5];
    let percentile = |values: &mut Vec<f32>, q: f32| -> f32 {
        if values.is_empty() {
            return f32::NAN;
        }
        values.sort_by(|a, b| a.total_cmp(b));
        values[((q * (values.len() - 1) as f32).round() as usize).min(values.len() - 1)]
    };
    println!("==== 崩壊の前触れ(総量 / 密度。健康なときを 1 とする。中央値) ====");
    for stressor in Stressor::STRESSES {
        let collapsed: Vec<&Episode> = episodes
            .iter()
            .filter(|e| e.stressor == stressor && e.collapse_step.is_some())
            .collect();
        let survived: Vec<&Episode> = episodes
            .iter()
            .filter(|e| e.stressor == stressor && e.collapse_step.is_none())
            .collect();
        let mut line = format!("  {:6} 崩壊 {:3} 本 |", stressor.label(), collapsed.len());
        for before in BEFORE_STEPS {
            let (mut masses, mut densities) = (Vec::new(), Vec::new());
            for episode in &collapsed {
                let collapse = episode.collapse_step.unwrap();
                let Some(target) = collapse.checked_sub(before) else {
                    continue;
                };
                if let Some(sample) = episode
                    .samples
                    .iter()
                    .rev()
                    .find(|s| s.step <= target && target - s.step < SAMPLE_EVERY)
                {
                    masses.push(sample.features[0]);
                    densities.push(sample.features[4]);
                }
            }
            line += &format!(
                " {:.1}秒前 {:.2}/{:.2}",
                before as f32 / STEPS_PER_SECOND,
                percentile(&mut masses, 0.5),
                percentile(&mut densities, 0.5)
            );
        }
        println!("{line}");
        let (mut lowest_masses, mut lowest_densities) = (Vec::new(), Vec::new());
        for episode in &survived {
            let lowest = |index: usize| {
                episode
                    .samples
                    .iter()
                    .map(|s| s.features[index])
                    .fold(f32::INFINITY, f32::min)
            };
            lowest_masses.push(lowest(0));
            lowest_densities.push(lowest(4));
        }
        println!(
            "  {:6} 生き延びた {:3} 本の底 | 総量 下位5% {:.2} / 中央値 {:.2} | 密度 下位5% {:.2} / 中央値 {:.2}",
            "",
            survived.len(),
            percentile(&mut lowest_masses, 0.05),
            percentile(&mut lowest_masses, 0.5),
            percentile(&mut lowest_densities, 0.05),
            percentile(&mut lowest_densities, 0.5),
        );
    }
    println!();
}
