//! 【実験】Lenia の場を、ほかの系を予測する計算の担い手(リザバー)として使えるか。
//!
//! これまでは「Lenia の場を予測する」実験をしてきた(docs/DESIGN.md「パラメータと場の両方から
//! 崩壊の確率を当てる」など)。その逆に、場そのものを計算資源として使う(リザバー
//! コンピューティング)。入力を体への弱い摂動として注ぎ、場の非線形な力学に混ぜさせ、
//! 場の状態から線形の読み出しだけを学習して、過去の入力を当てさせる
//! (docs/DESIGN.md「Lenia の場をリザバーとして使えるか」)。
//!
//! リザバーとして役に立つには、次の2つが要る。まずここだけを確かめる最小の実験。
//!
//! - **一貫性**: 同じ入力の列には、初期状態によらず同じように応える。カオス的な系では
//!   崩れうる。3通りの初期状態(普通に置いた / 位相をずらした / ごく小さなゆらぎを足した)に
//!   同じ入力を与え、場の状態が近づくか、1つめで学習した読み出しがほかでも使えるかを見る
//! - **記憶容量**: 1〜40 ステップ前の入力を読み出しでどれだけ復元できるか
//!   (相関の2乗の合計。リザバーの標準的な指標)
//!
//! 比べるのは、空の場(入力が減衰していくだけ)、生物のいる場(4種)、同じ大きさの
//! エコーステートネットワーク(256ユニット)。ペット本体の振る舞いには触れない。

use std::ops::Range;
use std::thread;
use std::time::Instant;

use vmc_pet_body::{load_animal, Animal, CellPos, Field, Lenia, Perturbation};

const FIELD_SIZE: usize = 32;
const CELLS: usize = FIELD_SIZE * FIELD_SIZE;
/// 読み出しに使う、重心を中心に切り出して 2×2 で縮めた場の一辺。
const CROP_SIDE: usize = 16;
const FEATURES: usize = CROP_SIDE * CROP_SIDE;

const WARMUP_STEPS: u32 = 60;
/// 位相をずらした初期状態のために、余分に進めるステップ数。
const EXTRA_PHASE_STEPS: u32 = 97;
const WASHOUT_STEPS: usize = 200;
const TRAIN_STEPS: usize = 3_000;
/// 学習区間の後ろのうち、正則化の強さを選ぶのに使う長さ。
const VALIDATION_STEPS: usize = 600;
const TEST_STEPS: usize = 1_000;
const TOTAL_STEPS: usize = WASHOUT_STEPS + TRAIN_STEPS + TEST_STEPS;
const MAX_DELAY: usize = 40;

/// 入力を注ぐ摂動の半径と、入力1あたりの強さ(毎ステップ注ぐ)。
const INPUT_RADIUS: f32 = 3.0;
const GAINS: [f32; 3] = [0.01, 0.03, 0.1];
const SEEDS: u64 = 3;
/// ごく小さなゆらぎの幅(各セルに足す)。
const NOISE: f32 = 1e-3;
const COLLAPSE_MASS: f32 = 5.0;
/// 総量が最初のこの倍を超えたら、膨張して形を失ったとみなす。
const EXPLOSION_RATIO: f32 = 3.0;
const RIDGE_LAMBDAS: [f64; 5] = [1e-3, 1e-1, 1e1, 1e3, 1e5];

const ESN_SIZE: usize = 256;
const ESN_DENSITY: f32 = 0.1;
const ESN_SPECTRAL_RADIUS: f64 = 0.9;

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

fn seed_for(text: &str, number: u64) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes().chain(number.to_le_bytes()) {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash | 1
}

/// 同じ入力の列を与えるときの、初期状態の作り方。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Start {
    /// 普通に置いて落ち着かせた状態。読み出しはこの走行で学習する。
    Placed,
    /// 余分に進めて位相をずらした状態(空の場やネットワークでは、ランダムな状態)。
    Shifted,
    /// `Placed` にごく小さなゆらぎを足した状態。
    Noisy,
}

/// 入力で駆動した系の、ステップごとの読み出し用の状態。
struct Run {
    states: Vec<Vec<f32>>,
    failure: Option<&'static str>,
}

/// 重心を中心に切り出して 2×2 で縮めた場。
fn crop(field: &Field, centroid: Option<(f32, f32)>) -> Vec<f32> {
    let view = field.view();
    let centre = FIELD_SIZE as f32 / 2.0;
    let (cx, cy) = centroid.unwrap_or((centre, centre));
    let (cx, cy) = (cx.round() as i32, cy.round() as i32);
    let size = FIELD_SIZE as i32;
    let half = size / 2;
    let mut out = Vec::with_capacity(FEATURES);
    for oy in 0..CROP_SIDE as i32 {
        for ox in 0..CROP_SIDE as i32 {
            let mut sum = 0.0f32;
            for dy in 0..2 {
                for dx in 0..2 {
                    let x = (cx - half + ox * 2 + dx).rem_euclid(size) as usize;
                    let y = (cy - half + oy * 2 + dy).rem_euclid(size) as usize;
                    sum += view.get(x, y);
                }
            }
            out.push(sum / 4.0);
        }
    }
    out
}

/// Lenia の場を入力で駆動する。`animal` が `None` なら空の場(O2u の規則で進める)。
fn drive_lenia(
    animal: Option<&Animal>,
    rule: &Animal,
    inputs: &[f32],
    gain: f32,
    start: Start,
    noise_seed: u64,
) -> Run {
    let mut lenia = Lenia::new(rule.params.clone());
    let mut field = Field::new(FIELD_SIZE, FIELD_SIZE);
    if let Some(animal) = animal {
        field.place_centered(&animal.pattern);
    }
    let warmup = WARMUP_STEPS
        + if start == Start::Shifted {
            EXTRA_PHASE_STEPS
        } else {
            0
        };
    for _ in 0..warmup {
        lenia.step(&mut field, 1.0);
    }
    let mut rng = Rng(noise_seed);
    match start {
        Start::Placed => {}
        Start::Shifted if animal.is_none() => {
            // 空の場は位相を持たないので、中央にランダムな小さな塊を置いて初期状態を変える
            let blob: Vec<f32> = (0..CELLS).map(|_| rng.range_f32(0.0, 0.3)).collect();
            field.map(|x, y, value| {
                let near_centre = (10..22).contains(&x) && (10..22).contains(&y);
                if near_centre {
                    blob[y * FIELD_SIZE + x]
                } else {
                    value
                }
            });
        }
        Start::Shifted => {}
        Start::Noisy => {
            let noise: Vec<f32> = (0..CELLS).map(|_| rng.range_f32(-NOISE, NOISE)).collect();
            field.map(|x, y, value| (value + noise[y * FIELD_SIZE + x]).clamp(0.0, 1.0));
        }
    }

    let reference_mass = field.mass();
    let mut states = Vec::with_capacity(inputs.len());
    for &input in inputs {
        let centroid = field.view().toroidal_centroid();
        let centre = FIELD_SIZE as f32 / 2.0;
        let (cx, cy) = centroid.unwrap_or((centre, centre));
        field.inject(&Perturbation {
            at: CellPos {
                x: (cx as usize).min(FIELD_SIZE - 1),
                y: (cy as usize).min(FIELD_SIZE - 1),
            },
            radius: INPUT_RADIUS,
            amount: gain * input,
        });
        lenia.step(&mut field, 1.0);
        if animal.is_some() {
            let mass = field.mass();
            if mass < COLLAPSE_MASS {
                return Run {
                    states,
                    failure: Some("崩壊"),
                };
            }
            if mass > EXPLOSION_RATIO * reference_mass {
                return Run {
                    states,
                    failure: Some("膨張"),
                };
            }
        }
        states.push(crop(&field, field.view().toroidal_centroid()));
    }
    Run {
        states,
        failure: None,
    }
}

/// 比べる相手の、エコーステートネットワーク。
struct Esn {
    weights: Vec<f64>,
    input: Vec<f64>,
}

impl Esn {
    fn new(seed: u64) -> Self {
        let mut rng = Rng(seed);
        let mut weights: Vec<f64> = (0..ESN_SIZE * ESN_SIZE)
            .map(|_| {
                if rng.unit() < ESN_DENSITY {
                    rng.range_f32(-1.0, 1.0) as f64
                } else {
                    0.0
                }
            })
            .collect();
        let input = (0..ESN_SIZE)
            .map(|_| rng.range_f32(-1.0, 1.0) as f64)
            .collect();
        // 冪乗法でスペクトル半径を見積もる(後ろ50回の伸び率の幾何平均)
        let mut vector = vec![1.0 / (ESN_SIZE as f64).sqrt(); ESN_SIZE];
        let mut log_growth = 0.0;
        for iteration in 0..200 {
            let next = multiply(&weights, &vector);
            let norm = next.iter().map(|v| v * v).sum::<f64>().sqrt().max(1e-12);
            if iteration >= 150 {
                log_growth += norm.ln() / 50.0;
            }
            vector = next.iter().map(|v| v / norm).collect();
        }
        let scale = ESN_SPECTRAL_RADIUS / log_growth.exp();
        for w in &mut weights {
            *w *= scale;
        }
        Esn { weights, input }
    }

    fn drive(&self, inputs: &[f32], start: Start, seed: u64) -> Run {
        let mut rng = Rng(seed);
        let mut state: Vec<f64> = match start {
            Start::Placed => vec![0.0; ESN_SIZE],
            Start::Shifted => (0..ESN_SIZE)
                .map(|_| rng.range_f32(-1.0, 1.0) as f64)
                .collect(),
            Start::Noisy => (0..ESN_SIZE)
                .map(|_| rng.range_f32(-NOISE, NOISE) as f64)
                .collect(),
        };
        let mut states = Vec::with_capacity(inputs.len());
        for &input in inputs {
            let recurrent = multiply(&self.weights, &state);
            state = recurrent
                .iter()
                .zip(&self.input)
                .map(|(r, w)| (r + w * (input as f64 - 0.5)).tanh())
                .collect();
            states.push(state.iter().map(|v| *v as f32).collect());
        }
        Run {
            states,
            failure: None,
        }
    }
}

fn multiply(matrix: &[f64], vector: &[f64]) -> Vec<f64> {
    matrix
        .chunks_exact(vector.len())
        .map(|row| row.iter().zip(vector).map(|(a, b)| a * b).sum())
        .collect()
}

/// リッジ回帰の正規方程式に要る量(特徴は標準化、目標は中心化する)。
struct Design {
    dim: usize,
    mean: Vec<f64>,
    std: Vec<f64>,
    gram: Vec<f64>,
    /// 遅れ k(1〜`MAX_DELAY`)ごとの、特徴と目標の積和。
    cross: Vec<Vec<f64>>,
    target_mean: Vec<f64>,
}

fn standardized(state: &[f32], mean: &[f64], std: &[f64], out: &mut [f64]) {
    for ((o, v), (m, s)) in out.iter_mut().zip(state).zip(mean.iter().zip(std)) {
        *o = (*v as f64 - m) / s;
    }
}

fn design(states: &[Vec<f32>], inputs: &[f32], range: Range<usize>) -> Design {
    let dim = states[0].len();
    let count = range.len() as f64;
    let mut mean = vec![0.0f64; dim];
    for t in range.clone() {
        for (m, v) in mean.iter_mut().zip(&states[t]) {
            *m += *v as f64 / count;
        }
    }
    let mut std = vec![0.0f64; dim];
    for t in range.clone() {
        for ((s, v), m) in std.iter_mut().zip(&states[t]).zip(&mean) {
            *s += (*v as f64 - m).powi(2) / count;
        }
    }
    for s in &mut std {
        *s = s.sqrt().max(1e-9);
    }
    let mut target_mean = vec![0.0f64; MAX_DELAY];
    for t in range.clone() {
        for (k, m) in target_mean.iter_mut().enumerate() {
            *m += inputs[t - k - 1] as f64 / count;
        }
    }
    let mut gram = vec![0.0f64; dim * dim];
    let mut cross = vec![vec![0.0f64; dim]; MAX_DELAY];
    let mut z = vec![0.0f64; dim];
    for t in range {
        standardized(&states[t], &mean, &std, &mut z);
        for (i, zi) in z.iter().enumerate() {
            if *zi == 0.0 {
                continue;
            }
            for (g, zj) in gram[i * dim..(i + 1) * dim].iter_mut().zip(&z) {
                *g += zi * zj;
            }
        }
        for (k, (c, m)) in cross.iter_mut().zip(&target_mean).enumerate() {
            let y = inputs[t - k - 1] as f64 - m;
            for (ci, zi) in c.iter_mut().zip(&z) {
                *ci += zi * y;
            }
        }
    }
    Design {
        dim,
        mean,
        std,
        gram,
        cross,
        target_mean,
    }
}

/// 下三角のコレスキー分解(その場で書き換える)。
#[allow(clippy::needless_range_loop)] // 行列の添字をそのまま書く方が読みやすい
fn cholesky(a: &mut [f64], n: usize) {
    for j in 0..n {
        let mut diagonal = a[j * n + j];
        for k in 0..j {
            diagonal -= a[j * n + k] * a[j * n + k];
        }
        let diagonal = diagonal.max(1e-12).sqrt();
        a[j * n + j] = diagonal;
        for i in (j + 1)..n {
            let mut sum = a[i * n + j];
            for k in 0..j {
                sum -= a[i * n + k] * a[j * n + k];
            }
            a[i * n + j] = sum / diagonal;
        }
    }
}

/// コレスキー分解した行列で `b` を解く(その場で書き換える)。
#[allow(clippy::needless_range_loop)] // 行列の添字をそのまま書く方が読みやすい
fn solve(l: &[f64], n: usize, b: &mut [f64]) {
    for i in 0..n {
        let mut sum = b[i];
        for k in 0..i {
            sum -= l[i * n + k] * b[k];
        }
        b[i] = sum / l[i * n + i];
    }
    for i in (0..n).rev() {
        let mut sum = b[i];
        for k in (i + 1)..n {
            sum -= l[k * n + i] * b[k];
        }
        b[i] = sum / l[i * n + i];
    }
}

fn ridge_weights(design: &Design, lambda: f64) -> Vec<Vec<f64>> {
    let n = design.dim;
    let mut matrix = design.gram.clone();
    for i in 0..n {
        matrix[i * n + i] += lambda;
    }
    cholesky(&mut matrix, n);
    design
        .cross
        .iter()
        .map(|c| {
            let mut w = c.clone();
            solve(&matrix, n, &mut w);
            w
        })
        .collect()
}

fn squared_correlation(a: &[f64], b: &[f64]) -> f64 {
    let n = a.len() as f64;
    let (mean_a, mean_b) = (a.iter().sum::<f64>() / n, b.iter().sum::<f64>() / n);
    let (mut covariance, mut var_a, mut var_b) = (0.0, 0.0, 0.0);
    for (x, y) in a.iter().zip(b) {
        covariance += (x - mean_a) * (y - mean_b);
        var_a += (x - mean_a).powi(2);
        var_b += (y - mean_b).powi(2);
    }
    if var_a <= 1e-12 || var_b <= 1e-12 {
        return 0.0;
    }
    covariance * covariance / (var_a * var_b)
}

/// 遅れごとの、復元の相関の2乗。
fn capacity_per_delay(
    design: &Design,
    weights: &[Vec<f64>],
    states: &[Vec<f32>],
    inputs: &[f32],
    range: Range<usize>,
) -> Vec<f64> {
    let mut z = vec![0.0f64; design.dim];
    let mut predictions: Vec<Vec<f64>> = (0..MAX_DELAY)
        .map(|_| Vec::with_capacity(range.len()))
        .collect();
    let mut targets: Vec<Vec<f64>> = (0..MAX_DELAY)
        .map(|_| Vec::with_capacity(range.len()))
        .collect();
    for t in range {
        standardized(&states[t], &design.mean, &design.std, &mut z);
        for (k, (w, m)) in weights.iter().zip(&design.target_mean).enumerate() {
            let prediction = m + w.iter().zip(&z).map(|(a, b)| a * b).sum::<f64>();
            predictions[k].push(prediction);
            targets[k].push(inputs[t - k - 1] as f64);
        }
    }
    predictions
        .iter()
        .zip(&targets)
        .map(|(p, y)| squared_correlation(p, y))
        .collect()
}

/// 2つの状態の相関(値の並びとしての近さ)。
fn state_similarity(a: &[f32], b: &[f32]) -> f64 {
    let a: Vec<f64> = a.iter().map(|v| *v as f64).collect();
    let b: Vec<f64> = b.iter().map(|v| *v as f64).collect();
    let n = a.len() as f64;
    let (mean_a, mean_b) = (a.iter().sum::<f64>() / n, b.iter().sum::<f64>() / n);
    let (mut covariance, mut var_a, mut var_b) = (0.0, 0.0, 0.0);
    for (x, y) in a.iter().zip(&b) {
        covariance += (x - mean_a) * (y - mean_b);
        var_a += (x - mean_a).powi(2);
        var_b += (y - mean_b).powi(2);
    }
    if var_a <= 1e-12 || var_b <= 1e-12 {
        return 0.0;
    }
    covariance / (var_a * var_b).sqrt()
}

struct Evaluation {
    capacity: f64,
    capacity_on_shifted: f64,
    capacity_on_noisy: f64,
    similarity_shifted: f64,
    similarity_noisy: f64,
    memory_length: usize,
    failure: Option<&'static str>,
}

fn evaluate(placed: &Run, shifted: &Run, noisy: &Run, inputs: &[f32]) -> Evaluation {
    let failure = placed.failure.or(shifted.failure).or(noisy.failure);
    if failure.is_some() {
        return Evaluation {
            capacity: f64::NAN,
            capacity_on_shifted: f64::NAN,
            capacity_on_noisy: f64::NAN,
            similarity_shifted: f64::NAN,
            similarity_noisy: f64::NAN,
            memory_length: 0,
            failure,
        };
    }
    let train_end = WASHOUT_STEPS + TRAIN_STEPS;
    let validation_start = train_end - VALIDATION_STEPS;
    let test = train_end..TOTAL_STEPS;

    let sub = design(&placed.states, inputs, WASHOUT_STEPS..validation_start);
    let validation_scores: Vec<f64> = RIDGE_LAMBDAS
        .iter()
        .map(|lambda| {
            let weights = ridge_weights(&sub, *lambda);
            capacity_per_delay(
                &sub,
                &weights,
                &placed.states,
                inputs,
                validation_start..train_end,
            )
            .iter()
            .sum()
        })
        .collect();
    let best = validation_scores
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map(|(index, _)| index)
        .unwrap();

    let full = design(&placed.states, inputs, WASHOUT_STEPS..train_end);
    let weights = ridge_weights(&full, RIDGE_LAMBDAS[best]);
    let per_delay = capacity_per_delay(&full, &weights, &placed.states, inputs, test.clone());
    let on = |run: &Run| -> f64 {
        capacity_per_delay(&full, &weights, &run.states, inputs, test.clone())
            .iter()
            .sum()
    };
    let similarity = |run: &Run| -> f64 {
        test.clone()
            .map(|t| state_similarity(&placed.states[t], &run.states[t]))
            .sum::<f64>()
            / test.len() as f64
    };
    Evaluation {
        capacity: per_delay.iter().sum(),
        capacity_on_shifted: on(shifted),
        capacity_on_noisy: on(noisy),
        similarity_shifted: similarity(shifted),
        similarity_noisy: similarity(noisy),
        memory_length: per_delay.iter().take_while(|r| **r >= 0.5).count(),
        failure: None,
    }
}

#[derive(Clone)]
enum Substrate {
    Empty,
    Creature(String),
    Esn,
}

impl Substrate {
    fn label(&self) -> String {
        match self {
            Substrate::Empty => "空の場".to_string(),
            Substrate::Creature(code) => code.clone(),
            Substrate::Esn => "ESN(256)".to_string(),
        }
    }
}

/// 1つの基質・入力の強さについて、入力の列を変えて評価する。
fn evaluate_substrate(substrate: &Substrate, gain: f32) -> Vec<Evaluation> {
    let rule = load_animal("O2u").unwrap();
    let animal = match substrate {
        Substrate::Creature(code) => Some(load_animal(code).unwrap()),
        _ => None,
    };
    (0..SEEDS)
        .map(|seed| {
            let mut rng = Rng(seed_for("inputs", seed));
            let inputs: Vec<f32> = (0..TOTAL_STEPS).map(|_| rng.unit()).collect();
            let label = substrate.label();
            let noise_seed = |start: u64| seed_for(&label, seed * 10 + start);
            let runs: Vec<Run> = [Start::Placed, Start::Shifted, Start::Noisy]
                .iter()
                .enumerate()
                .map(|(index, start)| match substrate {
                    Substrate::Esn => Esn::new(seed_for("esn", seed)).drive(
                        &inputs,
                        *start,
                        noise_seed(index as u64),
                    ),
                    Substrate::Empty | Substrate::Creature(_) => {
                        let rule = animal.as_ref().unwrap_or(&rule);
                        drive_lenia(
                            animal.as_ref(),
                            rule,
                            &inputs,
                            gain,
                            *start,
                            noise_seed(index as u64),
                        )
                    }
                })
                .collect();
            evaluate(&runs[0], &runs[1], &runs[2], &inputs)
        })
        .collect()
}

fn mean_of(values: impl Iterator<Item = f64>) -> f64 {
    let (sum, count) = values.fold((0.0, 0), |(s, c), v| (s + v, c + 1));
    if count == 0 {
        f64::NAN
    } else {
        sum / count as f64
    }
}

fn main() {
    let started = Instant::now();
    let mut substrates = vec![Substrate::Empty];
    substrates.extend(
        vmc_pet_body::list_animals()
            .unwrap()
            .into_iter()
            .map(|(code, _)| Substrate::Creature(code)),
    );
    substrates.push(Substrate::Esn);

    let results: Vec<(Substrate, Option<f32>, Vec<Evaluation>)> = thread::scope(|scope| {
        let handles: Vec<_> = substrates
            .iter()
            .map(|substrate| {
                scope.spawn(move || {
                    let gains: Vec<Option<f32>> = match substrate {
                        Substrate::Esn => vec![None],
                        _ => GAINS.iter().map(|g| Some(*g)).collect(),
                    };
                    gains
                        .into_iter()
                        .map(|gain| {
                            (
                                substrate.clone(),
                                gain,
                                evaluate_substrate(substrate, gain.unwrap_or(0.0)),
                            )
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().unwrap())
            .collect()
    });

    println!(
        "記憶容量(1〜{MAX_DELAY}ステップ前の入力の復元。相関の2乗の合計)と一貫性。入力の列 {SEEDS} 通りの平均"
    );
    println!("  記憶容量: 同じ走行 / 位相をずらした走行へ / ゆらぎを足した走行へ(読み出しは同じ走行で学習)");
    println!(
        "  状態の近さ: 位相をずらした走行 / ゆらぎを足した走行(同じ時刻の場の相関、評価区間の平均)"
    );
    println!("  記憶の長さ: 相関の2乗が 0.5 以上で復元できた遅れの数");
    println!();
    for (substrate, gain, evaluations) in &results {
        let failures: Vec<&str> = evaluations.iter().filter_map(|e| e.failure).collect();
        let ok: Vec<&Evaluation> = evaluations.iter().filter(|e| e.failure.is_none()).collect();
        let gain_label = gain.map_or("   -".to_string(), |g| format!("{g:.2}"));
        if ok.is_empty() {
            println!(
                "  {:8} 強さ {gain_label} | 全部失敗 {failures:?}",
                substrate.label()
            );
            continue;
        }
        println!(
            "  {:8} 強さ {gain_label} | 記憶容量 {:5.2} / {:5.2} / {:5.2} | 状態の近さ {:.3} / {:.3} | 記憶の長さ {:4.1} | 失敗 {}/{SEEDS} {failures:?}",
            substrate.label(),
            mean_of(ok.iter().map(|e| e.capacity)),
            mean_of(ok.iter().map(|e| e.capacity_on_shifted)),
            mean_of(ok.iter().map(|e| e.capacity_on_noisy)),
            mean_of(ok.iter().map(|e| e.similarity_shifted)),
            mean_of(ok.iter().map(|e| e.similarity_noisy)),
            mean_of(ok.iter().map(|e| e.memory_length as f64)),
            failures.len(),
        );
    }
    println!();
    println!("合計 {:.0} 秒", started.elapsed().as_secs_f32());
}
