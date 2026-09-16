//! 【実験】多チャンネル Lenia の生物は、Rust に移しても生きるか。ペットの場の大きさでも生きるか。
//!
//! 多チャンネル Lenia(Chan 2020, "Lenia and Expanded Universe")は、チャンネルごとに場を持ち、
//! カーネルごとに「どのチャンネルを読んで、どのチャンネルを育てるか」を決める。計算の重さは
//! docs/experiments/rule-candidates.md「多チャンネル Lenia の計算の重さ」で測ったが、生物は
//! 動かしていなかった。Chan 氏のリポジトリの `Python/found/{次元}{チャンネル数}{自己カーネル数}
//! {相互カーネル数}.json`(1行に1体の JSON)にある生物を読み、次の3つの条件で動かす。
//!
//! 1. 元の R のまま、128×128 の場(移植が合っているかの確認)
//! 2. R を 13(いまの体と同じ)に縮めて、64×64 の場
//! 3. R を 13 に縮めて、32×32 の場(PC 版のペットの場)
//!
//! 2 を挟むのは、R を縮めた影響と、場が狭い影響を分けて見るため。R の縮め方は Chan 氏の
//! `LeniaNDKC.py` の変換(`scipy.ndimage.zoom`、最近傍)に倣う。
//!
//! 更新式は `LeniaNDKC.py` の `calc_once` と同じにする: カーネル k ごとに、読むチャンネル c0 の場を
//! 畳み込んで成長関数 G_k を通し、育てるチャンネル c1 へ `dt·h_k·G_k` を足す。チャンネルごとに、
//! 足したカーネルの `h_k` の合計で割り、0〜1 に切り詰める。時間の刻み `dt = 1/T` と成長関数の形は
//! 最初のカーネルの値を使う。カーネルは相対半径 r・リングの重み b の多項式の輪で、合計が1になる
//! よう正規化する。Chan 氏の実装は FFT で畳み込むが、ここでは体と同じ疎な散布型で畳み込む
//! (場はトーラスなので結果は同じになる)。式・データの型・RLE の読み方・R の縮め方は crate の
//! `vmc_pet_body::multichannel` にあり(PC 版の `--preview-multichannel` と同じ実装)、このツールは
//! 分析と試験の処理だけを持つ。
//!
//! 使い方: `cargo run --release -p vmc-pet-body --example multichannel_trial -- <found の JSON が
//! あるディレクトリ> <出力先>`。ディレクトリには `found221.json` などの名前で置く。出力先に、
//! 最後の姿をチャンネルを色(赤・緑・青)に割り当てた PPM で書き出す。ペット本体には触れない。
//!
//! `-- <ディレクトリ> --suite <出力先>` で、R 13・64×64 で形を保った生物を、ペットの試験一式に
//! かける(docs/experiments/rule-candidates.md「多チャンネルの生物をペットの試験にかける」)。
//! 弱る 0.85・テンポ ×0.6・×1.6・クリック ×5 に加え、1つのチャンネルだけ成長を弱めて戻し、
//! チャンネルの配分(色の割合)が表情の軸になるかを見る。比べるため、1チャンネルの O2u も
//! 同じ手順にかける。
//!
//! `-- <ディレクトリ> --neglect <出力先>` で、同梱した9体(`assets/multichannel.json`)と O2u を、
//! 実際のペットと同じ流れで弱らせる(docs/experiments/rule-candidates.md「多チャンネルの生物を
//! ペットと同じ流れで放置する」): 2250 ステップかけて成長の強さを 0.85 へ、750 ステップかけて
//! テンポを ×0.6 へ下げ、20000 ステップ過ごしてから、750 ステップで元に戻して 3000 ステップ保つ。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread;
use std::time::Instant;

use vmc_pet_body::lenia::{GrowthMapping, KernelCore};
use vmc_pet_body::multichannel::{KernelData, MultiAnimal, MultiWorld, COLLAPSE_MASS};
use vmc_pet_body::{accumulate_into, load_animal, CellPos, Perturbation};

/// 読むファイル(`found{名前}.json`)。2チャンネル・自己1本、2チャンネル・自己2本、3チャンネル・自己1本。
const FILES: [&str; 3] = ["221", "222", "231"];
const STEPS: u32 = 3_000;
/// 速さを測る区間(最後の 300 ステップ)。
const SPEED_WINDOW: u32 = 300;
/// 途中の様子を記録するステップ。
const CHECKPOINT: u32 = 1_000;
/// どれかのチャンネルがこの値を超えるセルを、はっきり見える点とする。
const CLEARLY_VISIBLE: f32 = 0.2;
/// 塊を数えるときの目安と、主な塊とみなす総量の割合(`flexible_search.rs` と同じ)。
const BLOB_THRESHOLD: f32 = 0.1;
const MAJOR_BLOB_SHARE: f32 = 0.1;
/// 縮めた後の R。いまの体(O2u など)と同じ。
const PET_RADIUS: usize = 13;

/// (説明, 場の一辺, R を縮めるか)
const VARIANTS: [(&str, usize, bool); 3] = [
    ("元の R・128×128", 128, false),
    ("R 13・64×64", 64, true),
    ("R 13・32×32", 32, true),
];

/// 多チャンネルの場と生物は crate の実装を使う(PC 版の `--preview-multichannel` と同じ式)。
type World = MultiWorld;
type AnimalData = MultiAnimal;

/// 分析のための読み取り(crate の `MultiWorld` には持たせない、この実験ツールだけの処理)。
trait Analysis {
    fn visible_area(&self) -> usize;
    fn centroid(&self) -> Option<(f32, f32)>;
    fn major_blobs(&self) -> usize;
    fn write_ppm(&self, path: &Path);
}

impl Analysis for World {
    fn visible_area(&self) -> usize {
        (0..self.size * self.size)
            .filter(|&i| self.channels.iter().any(|c| c[i] > CLEARLY_VISIBLE))
            .count()
    }

    /// 全チャンネルの和の、トーラス上の重心(円周上の平均)。
    fn centroid(&self) -> Option<(f32, f32)> {
        let size = self.size as f32;
        let (mut cx, mut sx, mut cy, mut sy, mut total) = (0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32);
        for index in 0..self.size * self.size {
            let value = self.total(index);
            if value <= 0.0 {
                continue;
            }
            let ax = (index % self.size) as f32 / size * std::f32::consts::TAU;
            let ay = (index / self.size) as f32 / size * std::f32::consts::TAU;
            cx += value * ax.cos();
            sx += value * ax.sin();
            cy += value * ay.cos();
            sy += value * ay.sin();
            total += value;
        }
        if total <= 1e-6 {
            return None;
        }
        let angle = |c: f32, s: f32| {
            s.atan2(c).rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU * size
        };
        Some((angle(cx, sx), angle(cy, sy)))
    }

    /// 主な塊の数(全チャンネルの和で数える)。
    fn major_blobs(&self) -> usize {
        let size = self.size;
        let cells = size * size;
        let mut seen = vec![false; cells];
        let mut masses = Vec::new();
        for start in 0..cells {
            if seen[start] || self.total(start) <= BLOB_THRESHOLD {
                continue;
            }
            seen[start] = true;
            let mut stack = vec![start];
            let mut mass = 0.0;
            while let Some(index) = stack.pop() {
                mass += self.total(index);
                let (x, y) = (index % size, index / size);
                for (nx, ny) in [
                    ((x + 1) % size, y),
                    ((x + size - 1) % size, y),
                    (x, (y + 1) % size),
                    (x, (y + size - 1) % size),
                ] {
                    let neighbour = ny * size + nx;
                    if !seen[neighbour] && self.total(neighbour) > BLOB_THRESHOLD {
                        seen[neighbour] = true;
                        stack.push(neighbour);
                    }
                }
            }
            masses.push(mass);
        }
        let total: f32 = masses.iter().sum();
        masses
            .iter()
            .filter(|m| **m >= total * MAJOR_BLOB_SHARE)
            .count()
    }

    fn write_ppm(&self, path: &Path) {
        let scale = if self.size <= 32 {
            4
        } else if self.size <= 64 {
            2
        } else {
            1
        };
        let side = self.size * scale;
        let mut bytes = format!("P6\n{side} {side}\n255\n").into_bytes();
        for y in 0..side {
            for x in 0..side {
                let index = (y / scale) * self.size + x / scale;
                for channel in 0..3 {
                    let value = self.channels.get(channel).map_or(0.0, |c| c[index]);
                    bytes.push((value.clamp(0.0, 1.0) * 255.0).round() as u8);
                }
            }
        }
        fs::write(path, bytes).expect("PPM を書き出せない");
    }
}

fn toroidal_offset(offset: f32, size: f32) -> f32 {
    let offset = offset.rem_euclid(size);
    if offset > size / 2.0 {
        offset - size
    } else {
        offset
    }
}

/// 生物を条件に合わせて場に置く。`shrink` なら R を 13 に縮める。場に収まらなければ `None`。
fn place(animal: &AnimalData, size: usize, shrink: bool) -> Option<World> {
    let radius = if shrink {
        PET_RADIUS
    } else {
        animal.params[0].radius
    };
    MultiWorld::place(animal, size, radius)
}

/// カーネルのパラメータ(全体の R は `radius`)と、チャンネルごとの場の値から世界を作る。
fn build_world(
    params: &[KernelData],
    radius: usize,
    channels: Vec<Vec<f32>>,
    size: usize,
) -> World {
    MultiWorld::new(params, radius, channels, size)
}

/// 1つの条件での結末。
struct Outcome {
    /// "生存" / "崩壊" / "膨張" / "置けない"
    fate: String,
    /// 壊れたステップ。
    broken_at: Option<u32>,
    /// 最後(または壊れる直前)の総量 ÷ 置いたときの総量。
    mass_ratio: f32,
    /// チャンネルごとの最後の総量 ÷ 置いたときの総量。
    channel_ratios: Vec<f32>,
    /// 1000 ステップ目と最後の、主な塊の数。
    blobs: [usize; 2],
    /// 最後の 300 ステップの速さ(1ステップあたりの重心の移動)。
    speed: f32,
}

fn run(animal: &AnimalData, size: usize, shrink: bool, image: &Path) -> Outcome {
    let Some(mut world) = place(animal, size, shrink) else {
        return Outcome {
            fate: "置けない".into(),
            broken_at: None,
            mass_ratio: 0.0,
            channel_ratios: Vec::new(),
            blobs: [0, 0],
            speed: 0.0,
        };
    };
    let initial = world.mass().max(1e-6);
    let initial_channels = world.channel_masses();
    let mut outcome = Outcome {
        fate: "生存".into(),
        broken_at: None,
        mass_ratio: 1.0,
        channel_ratios: Vec::new(),
        blobs: [0, 0],
        speed: 0.0,
    };
    let (mut path, mut moves) = (0.0f32, 0u32);
    let mut previous = None;
    for step in 1..=STEPS {
        world.step();
        let mass = world.mass();
        if mass < COLLAPSE_MASS {
            outcome.fate = "崩壊".into();
            outcome.broken_at = Some(step);
            break;
        }
        if world.visible_area() > size * size / 2 {
            outcome.fate = "膨張".into();
            outcome.broken_at = Some(step);
            break;
        }
        if step == CHECKPOINT {
            outcome.blobs[0] = world.major_blobs();
        }
        if step > STEPS - SPEED_WINDOW {
            let current = world.centroid();
            if let (Some(a), Some(b)) = (previous, current) {
                let (a, b): ((f32, f32), (f32, f32)) = (a, b);
                let (dx, dy) = (
                    toroidal_offset(b.0 - a.0, size as f32),
                    toroidal_offset(b.1 - a.1, size as f32),
                );
                path += (dx * dx + dy * dy).sqrt();
                moves += 1;
            }
            previous = current;
        }
    }
    outcome.mass_ratio = world.mass() / initial;
    outcome.channel_ratios = world
        .channel_masses()
        .iter()
        .zip(&initial_channels)
        .map(|(now, first)| now / first.max(1e-6))
        .collect();
    outcome.blobs[1] = world.major_blobs();
    outcome.speed = path / moves.max(1) as f32;
    world.write_ppm(image);
    outcome
}

fn load(directory: &Path, name: &str) -> Vec<AnimalData> {
    let text = fs::read_to_string(directory.join(format!("found{name}.json")))
        .unwrap_or_else(|_| panic!("found{name}.json を読めない"));
    // 1行に1体で、行末に「,」が付いている(全体を1つの配列としては読めない)
    text.lines()
        .map(|line| line.trim().trim_end_matches(','))
        .filter(|line| line.starts_with('{'))
        .map(|line| serde_json::from_str::<AnimalData>(line).expect("生物の JSON が壊れている"))
        .collect()
}

fn format_outcome(outcome: &Outcome) -> String {
    match outcome.fate.as_str() {
        "置けない" => "置けない".to_string(),
        "生存" => {
            let channels: Vec<String> = outcome
                .channel_ratios
                .iter()
                .map(|r| format!("{r:.2}"))
                .collect();
            format!(
                "生存 総量×{:.2} [{}] 塊 {}→{} 速さ {:.3}",
                outcome.mass_ratio,
                channels.join(" "),
                outcome.blobs[0],
                outcome.blobs[1],
                outcome.speed
            )
        }
        fate => format!(
            "{fate} {} ステップ目",
            outcome.broken_at.map_or("-".into(), |s| s.to_string())
        ),
    }
}

/// 照合用: 元の R・128×128 で動かし、チャンネルごとの総量を決まったステップで出す。numpy で
/// `LeniaNDKC.py` の式をそのまま写した参照実装(倍精度・FFT)と、最初の数ステップを突き合わせる
/// ため。カオスなので長く動かすと浮動小数点の差で分かれる。
fn trace(directory: &Path, file: &str, index: usize) {
    const TRACE_STEPS: [u32; 5] = [1, 10, 100, 300, 1000];
    let animal = &load(directory, file)[index];
    let mut world = place(animal, 128, false).expect("元の R では 128×128 に置けるはず");
    let masses = |world: &World| {
        world
            .channel_masses()
            .iter()
            .map(|m| format!("{m:.6}"))
            .collect::<Vec<_>>()
            .join(" ")
    };
    println!("{file}#{index:02} 置いたとき {}", masses(&world));
    for step in 1..=TRACE_STEPS[TRACE_STEPS.len() - 1] {
        world.step();
        if TRACE_STEPS.contains(&step) {
            println!("{file}#{index:02} {step:>4} ステップ {}", masses(&world));
        }
    }
}

// ==== 試験の一式(--suite) ====

/// 試験にかける場の一辺。R 13 で形を保つ生物が多かった 64×64。
const SUITE_FIELD: usize = 64;
/// 落ち着かせる長さ(形を保つかの判定は、生物を動かした検証の 3000 ステップと同じ)。
const SETTLE_STEPS: u32 = 3_000;
const MEASURE_STEPS: u32 = 900;
const RAMP_STEPS: u32 = 750;
const HOLD_STEPS: u32 = 1_500;
const RETURN_HOLD_STEPS: u32 = 3_000;
const SUITE_TRIALS: u64 = 2;
const NOISE: f32 = 1e-3;
/// 実際のクリックと同じ半径と量(`touch.rs`)を、1秒ごとに5回、全チャンネルへ注入する。
const CLICK_RADIUS: f32 = 4.5;
const CLICK_AMOUNT: f32 = 0.20;
const CLICK_COUNT: u32 = 5;
const CLICK_INTERVAL_STEPS: u32 = 15;
/// 放置されて弱りきった体の成長の強さ(`MIN_GROWTH_SCALE`)。
const ENERGY_FLOOR: f32 = 0.85;
/// 見た目の差の相対差の分母の下限と、戻ったとみなす差(`flexible_search.rs` と同じ)。
const SPEED_FLOOR: f32 = 0.05;
const MASS_DEVIATION_FLOOR: f32 = 0.5;
const AREA_FLOOR: f32 = 5.0;
const RETURNED_DISTANCE: f32 = 0.3;
/// 色の差(チャンネルの割合の差の最大)がこれ未満なら、色も戻ったとみなす。
const RETURNED_COLOUR: f32 = 0.05;
/// 形を保つとみなす総量の比の範囲と、チャンネルごとの残りの下限(生物を動かした検証と同じ)。
const FORM_MASS_RANGE: (f32, f32) = (0.8, 1.25);
const FORM_CHANNEL_MIN: f32 = 0.5;

#[derive(Clone, Copy)]
enum SuiteTest {
    /// 全チャンネルの成長を弱める(放置されて弱る)。
    Energy,
    /// テンポを変える(がっかり ×0.6、速い ×1.6)。
    Tempo(f32),
    /// 実際のクリックを重心に5回。
    Clicks,
    /// 1つのチャンネルだけ成長を弱める。チャンネルの配分(色の割合)が表情の軸になるかを見る。
    ChannelEnergy(usize),
}

impl SuiteTest {
    fn label(self) -> String {
        match self {
            Self::Energy => "弱る".into(),
            Self::Tempo(tempo) => format!("テンポ×{tempo}"),
            Self::Clicks => "クリック".into(),
            Self::ChannelEnergy(channel) => format!("ch{channel}だけ弱る"),
        }
    }

    /// ペットがいま体に与える変化か(合格の条件に使う)。
    fn is_pet_test(self) -> bool {
        !matches!(self, Self::ChannelEnergy(_))
    }
}

/// 60秒ぶんの見た目の特徴。
#[derive(Clone)]
struct Features {
    speed: f32,
    mass_deviation: f32,
    area: f32,
    /// チャンネルごとの総量の割合(色の割合)。
    shares: Vec<f32>,
}

fn floored_difference(a: f32, b: f32, floor: f32) -> f32 {
    (a - b).abs() / a.abs().max(b.abs()).max(floor)
}

/// 見た目の差(動き差 + 形差)。
fn distance(a: &Features, b: &Features) -> f32 {
    let motion = (floored_difference(a.speed, b.speed, SPEED_FLOOR)
        + floored_difference(a.mass_deviation, b.mass_deviation, MASS_DEVIATION_FLOOR))
        / 2.0;
    motion + floored_difference(a.area, b.area, AREA_FLOOR)
}

/// 色の差(チャンネルの割合の差の最大)。
fn colour_difference(a: &Features, b: &Features) -> f32 {
    a.shares
        .iter()
        .zip(&b.shares)
        .map(|(x, y)| (x - y).abs())
        .fold(0.0, f32::max)
}

/// 試験のための操作(この実験ツールだけの処理)。
trait SuiteWorld {
    fn broken(&self) -> bool;
    fn measure(
        &mut self,
        steps: u32,
        scales: &[f32],
        tempo: f32,
        before_step: impl FnMut(&mut World, u32),
    ) -> Option<Features>;
    fn ramp(&mut self, steps: u32, from: (&[f32], f32), to: (&[f32], f32)) -> bool;
    fn click(&mut self);
}

impl SuiteWorld for World {
    fn broken(&self) -> bool {
        self.mass() < COLLAPSE_MASS || self.visible_area() > self.size * self.size / 2
    }

    /// `steps` ステップ進め、最後の `MEASURE_STEPS`(足りなければ全部)で特徴を測る。
    /// `before_step` はステップの前に呼ぶ(クリックに使う)。壊れたら `None`。
    fn measure(
        &mut self,
        steps: u32,
        scales: &[f32],
        tempo: f32,
        mut before_step: impl FnMut(&mut World, u32),
    ) -> Option<Features> {
        let size = self.size as f32;
        let (mut path, mut moves) = (0.0f32, 0u32);
        let mut previous: Option<(f32, f32)> = None;
        let mut masses = Vec::new();
        let mut area = 0.0f32;
        let mut shares = vec![0.0f32; self.channels.len()];
        let start = steps.saturating_sub(MEASURE_STEPS);
        for step in 0..steps {
            before_step(self, step);
            self.step_with(scales, tempo);
            if self.broken() {
                return None;
            }
            if step < start {
                continue;
            }
            let current = self.centroid();
            if let (Some(a), Some(b)) = (previous, current) {
                let (dx, dy) = (
                    toroidal_offset(b.0 - a.0, size),
                    toroidal_offset(b.1 - a.1, size),
                );
                path += (dx * dx + dy * dy).sqrt();
                moves += 1;
            }
            previous = current;
            let channel_masses = self.channel_masses();
            let total: f32 = channel_masses.iter().sum();
            for (share, mass) in shares.iter_mut().zip(&channel_masses) {
                *share += mass / total.max(1e-6);
            }
            masses.push(total);
            area += self.visible_area() as f32;
        }
        let count = masses.len().max(1) as f32;
        let mean = masses.iter().sum::<f32>() / count;
        let variance = masses.iter().map(|m| (m - mean) * (m - mean)).sum::<f32>() / count;
        Some(Features {
            speed: path / moves.max(1) as f32,
            mass_deviation: variance.sqrt(),
            area: area / count,
            shares: shares.iter().map(|s| s / count).collect(),
        })
    }

    /// 成長の強さとテンポを、`from` から `to` へ `steps` ステップかけて動かす。壊れたら `false`。
    fn ramp(&mut self, steps: u32, from: (&[f32], f32), to: (&[f32], f32)) -> bool {
        for step in 0..steps {
            let progress = step as f32 / steps as f32;
            let scales: Vec<f32> = from
                .0
                .iter()
                .zip(to.0)
                .map(|(a, b)| a + (b - a) * progress)
                .collect();
            let tempo = from.1 + (to.1 - from.1) * progress;
            self.step_with(&scales, tempo);
            if self.broken() {
                return false;
            }
        }
        true
    }

    /// 全チャンネルの和の重心へ、実際のクリックと同じ山型の摂動を全チャンネルに注入する。
    fn click(&mut self) {
        let Some((x, y)) = self.centroid() else {
            return;
        };
        let size = self.size;
        let perturbation = Perturbation {
            at: CellPos {
                x: (x.round() as usize) % size,
                y: (y.round() as usize) % size,
            },
            radius: CLICK_RADIUS,
            amount: CLICK_AMOUNT,
        };
        for channel in &mut self.channels {
            accumulate_into(channel, size, size, &perturbation, 0.0, 1.0);
        }
    }
}

/// 1つの試験・1回の結果。
#[derive(Clone, Copy)]
struct TestResult {
    expressed: f32,
    expressed_colour: f32,
    returned: f32,
    returned_colour: f32,
}

impl TestResult {
    fn returned_home(self) -> bool {
        self.returned < RETURNED_DISTANCE && self.returned_colour < RETURNED_COLOUR
    }
}

/// 候補の姿 `state` から試験を1つ行う。`snapshot` があれば、変化をかけ終えたときの姿を書き出す。
fn run_suite_test(
    params: &[KernelData],
    radius: usize,
    state: &[Vec<f32>],
    baseline: &Features,
    test: SuiteTest,
    snapshot: Option<&Path>,
) -> Option<TestResult> {
    let mut world = build_world(params, radius, state.to_vec(), SUITE_FIELD);
    let normal = vec![1.0; world.channels.len()];
    let expressed = match test {
        SuiteTest::Clicks => world.measure(MEASURE_STEPS, &normal, 1.0, |world, step| {
            let clicking = step.is_multiple_of(CLICK_INTERVAL_STEPS)
                && step / CLICK_INTERVAL_STEPS < CLICK_COUNT;
            if clicking {
                world.click();
            }
        })?,
        _ => {
            let (scales, tempo) = match test {
                SuiteTest::Energy => (vec![ENERGY_FLOOR; normal.len()], 1.0),
                SuiteTest::Tempo(tempo) => (normal.clone(), tempo),
                SuiteTest::ChannelEnergy(channel) => {
                    let mut scales = normal.clone();
                    scales[channel] = ENERGY_FLOOR;
                    (scales, 1.0)
                }
                SuiteTest::Clicks => unreachable!(),
            };
            if !world.ramp(RAMP_STEPS, (&normal, 1.0), (&scales, tempo)) {
                return None;
            }
            let features = world.measure(HOLD_STEPS, &scales, tempo, |_, _| {})?;
            if let Some(path) = snapshot {
                world.write_ppm(path);
            }
            if !world.ramp(RAMP_STEPS, (&scales, tempo), (&normal, 1.0)) {
                return None;
            }
            features
        }
    };
    let returned = world.measure(RETURN_HOLD_STEPS, &normal, 1.0, |_, _| {})?;
    Some(TestResult {
        expressed: distance(&expressed, baseline),
        expressed_colour: colour_difference(&expressed, baseline),
        returned: distance(&returned, baseline),
        returned_colour: colour_difference(&returned, baseline),
    })
}

/// 試験にかける生物(見つかった生物、または比べるための1チャンネルの生物)。
struct SuiteSubject {
    label: String,
    params: Vec<KernelData>,
    radius: usize,
    channels: Vec<Vec<f32>>,
}

/// 1体の試験の結果。
struct SuiteOutcome {
    label: String,
    channel_count: usize,
    /// 候補になれなかった理由。候補なら `None`。
    rejection: Option<String>,
    baseline: Option<Features>,
    noise: (f32, f32),
    /// (試験, 試行ごとの結果)
    tests: Vec<(SuiteTest, Vec<Option<TestResult>>)>,
}

fn run_suite(subject: &SuiteSubject, output: &Path) -> SuiteOutcome {
    let mut outcome = SuiteOutcome {
        label: subject.label.clone(),
        channel_count: subject.channels.len(),
        rejection: None,
        baseline: None,
        noise: (0.0, 0.0),
        tests: Vec::new(),
    };
    let mut world = build_world(
        &subject.params,
        subject.radius,
        subject.channels.clone(),
        SUITE_FIELD,
    );
    let initial_total = world.mass().max(1e-6);
    let initial_channels = world.channel_masses();
    for step in 1..=SETTLE_STEPS {
        world.step();
        if world.broken() {
            outcome.rejection = Some(format!("落ち着かせる間に壊れた({step} ステップ目)"));
            return outcome;
        }
    }
    let ratio = world.mass() / initial_total;
    let channel_min = world
        .channel_masses()
        .iter()
        .zip(&initial_channels)
        .map(|(now, first)| now / first.max(1e-6))
        .fold(f32::INFINITY, f32::min);
    let blobs = world.major_blobs();
    let keeps_form = (FORM_MASS_RANGE.0..=FORM_MASS_RANGE.1).contains(&ratio)
        && channel_min >= FORM_CHANNEL_MIN
        && blobs == 1;
    if !keeps_form {
        outcome.rejection = Some(format!(
            "形を保たない(総量×{ratio:.2}・チャンネルの残りの最小 {channel_min:.2}・主な塊 {blobs})"
        ));
        return outcome;
    }
    let file_label = subject.label.replace([' ', '#', ','], "_");
    let settled = world.channels.clone();

    let mut baselines = Vec::new();
    let mut states = Vec::new();
    for trial in 0..SUITE_TRIALS {
        let mut rng = trial.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0x5EED_0000_0000_0001;
        let mut noise = || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            ((rng >> 11) as f32 / (1u64 << 53) as f32 * 2.0 - 1.0) * NOISE
        };
        let noisy: Vec<Vec<f32>> = settled
            .iter()
            .map(|c| c.iter().map(|v| (v * (1.0 + noise())).min(1.0)).collect())
            .collect();
        let mut trial_world = build_world(&subject.params, subject.radius, noisy, SUITE_FIELD);
        let normal = vec![1.0; settled.len()];
        let Some(baseline) = trial_world.measure(MEASURE_STEPS, &normal, 1.0, |_, _| {}) else {
            outcome.rejection = Some("基準を測る間に壊れた".into());
            return outcome;
        };
        if trial == 0 {
            trial_world.write_ppm(&output.join(format!("{file_label}_base.ppm")));
        }
        baselines.push(baseline);
        states.push(trial_world.channels);
    }
    outcome.noise = (
        distance(&baselines[0], &baselines[1]),
        colour_difference(&baselines[0], &baselines[1]),
    );
    let mut tests = vec![
        SuiteTest::Energy,
        SuiteTest::Tempo(0.6),
        SuiteTest::Tempo(1.6),
        SuiteTest::Clicks,
    ];
    if settled.len() > 1 {
        tests.extend((0..settled.len()).map(SuiteTest::ChannelEnergy));
    }
    for test in tests {
        let results = (0..SUITE_TRIALS as usize)
            .map(|trial| {
                let snapshot = (trial == 0 && !matches!(test, SuiteTest::Clicks))
                    .then(|| output.join(format!("{file_label}_{}.ppm", test.label())));
                run_suite_test(
                    &subject.params,
                    subject.radius,
                    &states[trial],
                    &baselines[trial],
                    test,
                    snapshot.as_deref(),
                )
            })
            .collect();
        outcome.tests.push((test, results));
    }
    outcome.baseline = Some(baselines.swap_remove(0));
    outcome
}

/// 比べるための1チャンネルの生物(vmc-pet の animals.json)を、試験にかけられる形にする。
fn single_channel_subject(code: &str) -> SuiteSubject {
    let animal = load_animal(code).unwrap();
    assert_eq!(animal.params.kernel_core, KernelCore::Polynomial);
    assert_eq!(animal.params.growth_mapping, GrowthMapping::Polynomial);
    let peaks: Vec<String> = animal
        .params
        .kernel_peaks
        .iter()
        .map(|p| p.to_string())
        .collect();
    let params = vec![KernelData {
        radius: animal.params.radius,
        time_divisor: animal.params.time_divisor,
        b: peaks.join(","),
        m: animal.params.growth_center,
        s: animal.params.growth_width,
        h: 1.0,
        r: 1.0,
        kn: 1,
        gn: 1,
        c: [0, 0],
    }];
    let (width, height) = (animal.pattern.width(), animal.pattern.height());
    let (left, top) = ((SUITE_FIELD - width) / 2, (SUITE_FIELD - height) / 2);
    let mut field = vec![0.0; SUITE_FIELD * SUITE_FIELD];
    for y in 0..height {
        for x in 0..width {
            field[(top + y) * SUITE_FIELD + left + x] = animal.pattern.get(x, y);
        }
    }
    SuiteSubject {
        label: format!("比較 {code}(1チャンネル)"),
        params,
        radius: animal.params.radius,
        channels: vec![field],
    }
}

fn format_test(results: &[Option<TestResult>]) -> String {
    let Some(all) = results.iter().copied().collect::<Option<Vec<TestResult>>>() else {
        return format!("{:>11}", "崩壊");
    };
    let n = all.len() as f32;
    let expressed = all.iter().map(|r| r.expressed).sum::<f32>() / n;
    let colour = all.iter().map(|r| r.expressed_colour).sum::<f32>() / n;
    let mark = if all.iter().all(|r| r.returned_home()) {
        " "
    } else {
        "✗"
    };
    format!("{expressed:>4.2}/{colour:>4.2}{mark}")
}

fn suite(directory: &Path, output: &Path) {
    fs::create_dir_all(output).expect("出力先を作れない");
    let started = Instant::now();
    let mut subjects = vec![single_channel_subject("O2u")];
    for name in FILES {
        for (index, animal) in load(directory, name).into_iter().enumerate() {
            let label = format!("{name} #{index:02} {}", animal.name)
                .trim()
                .to_string();
            let shrunk = place(&animal, SUITE_FIELD, true);
            let Some(world) = shrunk else {
                continue;
            };
            subjects.push(SuiteSubject {
                label,
                params: animal.params.clone(),
                radius: PET_RADIUS,
                channels: world.channels,
            });
        }
    }
    let job_count = subjects.len();
    let queue = Mutex::new((0..job_count).collect::<Vec<_>>().into_iter());
    let results: Mutex<Vec<Option<SuiteOutcome>>> =
        Mutex::new((0..job_count).map(|_| None).collect());
    let workers = thread::available_parallelism().map_or(4, |n| n.get());
    thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let Some(index) = queue.lock().unwrap().next() else {
                    break;
                };
                let outcome = run_suite(&subjects[index], output);
                results.lock().unwrap()[index] = Some(outcome);
            });
        }
    });
    let results: Vec<SuiteOutcome> = results
        .into_inner()
        .unwrap()
        .into_iter()
        .map(Option::unwrap)
        .collect();

    println!(
        "場 {SUITE_FIELD}×{SUITE_FIELD}・R {PET_RADIUS}、各試験 {SUITE_TRIALS} 回。数字は変化をかけている間の「見た目の差/色の差」(試行の平均)"
    );
    println!(
        "✗ は戻らなかった試行がある(見た目の差 {RETURNED_DISTANCE} 以上か色の差 {RETURNED_COLOUR} 以上)。合格 = ペットの4試験で壊れず戻った"
    );
    let mut rejected = 0;
    let mut passed = Vec::new();
    for outcome in &results {
        let Some(baseline) = &outcome.baseline else {
            rejected += 1;
            continue;
        };
        let pet_pass = outcome
            .tests
            .iter()
            .filter(|(test, _)| test.is_pet_test())
            .all(|(_, results)| results.iter().all(|r| r.is_some_and(|r| r.returned_home())));
        if pet_pass {
            passed.push(outcome.label.clone());
        }
        let shares: Vec<String> = baseline.shares.iter().map(|s| format!("{s:.2}")).collect();
        let tests: Vec<String> = outcome
            .tests
            .iter()
            .map(|(test, results)| format!("{} {}", test.label(), format_test(results)))
            .collect();
        println!(
            "{}{} | ch{} 速さ {:.3} 点 {:.0} 色の割合 [{}] ゆらぎ {:.2}/{:.2}",
            outcome.label,
            if pet_pass { "  合格" } else { "" },
            outcome.channel_count,
            baseline.speed,
            baseline.area,
            shares.join(" "),
            outcome.noise.0,
            outcome.noise.1
        );
        println!("    {}", tests.join(" | "));
    }
    println!();
    println!(
        "候補 {} 体(形を保たなかった・壊れた {rejected} 体)、合格 {} 体: {}",
        results.len() - rejected,
        passed.len(),
        passed.join("、")
    );
    for outcome in results.iter().filter(|o| o.rejection.is_some()) {
        println!(
            "  候補外: {} {}",
            outcome.label,
            outcome.rejection.as_deref().unwrap_or("")
        );
    }
    println!("合計 {:.0} 秒", started.elapsed().as_secs_f32());
}

// ==== 実際のペットと同じ流れで放置する(--neglect) ====

/// 触られないままエネルギーが尽きるまでのステップ数(`LeniaBody` の減り方と同じ約150秒)。
const NEGLECT_STEPS: u32 = 2_250;
/// エネルギーが尽きてから、がっかりを最大にする(テンポを下げる)までのステップ数。
const MOOD_RAMP_STEPS: u32 = 750;
/// がっかりしきったまま過ごすステップ数(docs/DESIGN.md で1チャンネルの4種を確かめた長さ)。
const NEGLECTED_HOLD_STEPS: u32 = 20_000;
/// 世話されて元に戻る流れの長さ。
const RECOVER_STEPS: u32 = 750;
const NEGLECT_TRIALS: u64 = 8;
/// がっかりしきったときのテンポ(`mood::DISAPPOINTED_TEMPO`)。
const DISAPPOINTED_TEMPO: f32 = 0.6;

/// 1回の放置の結果。
struct NeglectResult {
    /// 壊れた区間。壊れなければ `None`。
    broken_in: Option<&'static str>,
    /// がっかりしきって過ごした最後の 900 ステップの、基準との差(見た目, 色)。
    neglected: Option<(f32, f32)>,
    /// 戻して保った後の、基準との差(見た目, 色)。
    returned: Option<(f32, f32)>,
}

fn run_neglect(subject: &SuiteSubject, trial: u64, output: &Path) -> NeglectResult {
    let mut result = NeglectResult {
        broken_in: None,
        neglected: None,
        returned: None,
    };
    let mut world = build_world(
        &subject.params,
        subject.radius,
        subject.channels.clone(),
        SUITE_FIELD,
    );
    let normal = vec![1.0; world.channels.len()];
    for _ in 0..SETTLE_STEPS {
        world.step();
        if world.broken() {
            result.broken_in = Some("落ち着かせる間");
            return result;
        }
    }
    let mut rng = trial.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0x5EED_0000_0000_0002;
    for channel in &mut world.channels {
        for value in channel.iter_mut() {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            let noise = ((rng >> 11) as f32 / (1u64 << 53) as f32 * 2.0 - 1.0) * NOISE;
            *value = (*value * (1.0 + noise)).min(1.0);
        }
    }
    let Some(baseline) = world.measure(MEASURE_STEPS, &normal, 1.0, |_, _| {}) else {
        result.broken_in = Some("基準を測る間");
        return result;
    };
    let weak = vec![ENERGY_FLOOR; normal.len()];
    if !world.ramp(NEGLECT_STEPS, (&normal, 1.0), (&weak, 1.0)) {
        result.broken_in = Some("エネルギーが尽きる間");
        return result;
    }
    if !world.ramp(MOOD_RAMP_STEPS, (&weak, 1.0), (&weak, DISAPPOINTED_TEMPO)) {
        result.broken_in = Some("がっかりしていく間");
        return result;
    }
    let Some(neglected) = world.measure(NEGLECTED_HOLD_STEPS, &weak, DISAPPOINTED_TEMPO, |_, _| {})
    else {
        result.broken_in = Some("がっかりしきって過ごす間");
        return result;
    };
    result.neglected = Some((
        distance(&neglected, &baseline),
        colour_difference(&neglected, &baseline),
    ));
    let file_label = subject.label.replace([' ', '#', ',', '(', ')'], "_");
    if trial == 0 {
        world.write_ppm(&output.join(format!("{file_label}_neglected.ppm")));
    }
    if !world.ramp(RECOVER_STEPS, (&weak, DISAPPOINTED_TEMPO), (&normal, 1.0)) {
        result.broken_in = Some("元に戻る間");
        return result;
    }
    let Some(returned) = world.measure(RETURN_HOLD_STEPS, &normal, 1.0, |_, _| {}) else {
        result.broken_in = Some("戻して保つ間");
        return result;
    };
    result.returned = Some((
        distance(&returned, &baseline),
        colour_difference(&returned, &baseline),
    ));
    if trial == 0 {
        world.write_ppm(&output.join(format!("{file_label}_returned.ppm")));
    }
    result
}

fn neglect(output: &Path) {
    fs::create_dir_all(output).expect("出力先を作れない");
    let started = Instant::now();
    let mut subjects = vec![single_channel_subject("O2u")];
    for animal in vmc_pet_body::multichannel::list_multichannel().expect("同梱データが壊れている")
    {
        let world =
            MultiWorld::place(&animal, SUITE_FIELD, PET_RADIUS).expect("64×64 に置けるはず");
        subjects.push(SuiteSubject {
            label: format!("{} {}", animal.id, animal.name).trim().to_string(),
            params: animal.params.clone(),
            radius: PET_RADIUS,
            channels: world.channels,
        });
    }
    let jobs: Vec<(usize, u64)> = (0..subjects.len())
        .flat_map(|s| (0..NEGLECT_TRIALS).map(move |t| (s, t)))
        .collect();
    let job_count = jobs.len();
    let queue = Mutex::new(jobs.into_iter().enumerate());
    let results: Mutex<Vec<Option<NeglectResult>>> =
        Mutex::new((0..job_count).map(|_| None).collect());
    let workers = thread::available_parallelism().map_or(4, |n| n.get());
    thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let Some((index, (subject, trial))) = queue.lock().unwrap().next() else {
                    break;
                };
                let result = run_neglect(&subjects[subject], trial, output);
                results.lock().unwrap()[index] = Some(result);
            });
        }
    });
    let results: Vec<NeglectResult> = results
        .into_inner()
        .unwrap()
        .into_iter()
        .map(Option::unwrap)
        .collect();

    println!(
        "場 {SUITE_FIELD}×{SUITE_FIELD}・R {PET_RADIUS}、各 {NEGLECT_TRIALS} 回。成長の強さを {NEGLECT_STEPS} ステップで {ENERGY_FLOOR} へ、テンポを {MOOD_RAMP_STEPS} ステップで ×{DISAPPOINTED_TEMPO} へ下げ、{NEGLECTED_HOLD_STEPS} ステップ過ごし、{RECOVER_STEPS} ステップで戻して {RETURN_HOLD_STEPS} ステップ保つ"
    );
    println!(
        "差は基準との「見た目の差/色の差」(生き延びた試行の平均)。戻った = 見た目の差 {RETURNED_DISTANCE} 未満かつ色の差 {RETURNED_COLOUR} 未満"
    );
    for (index, subject) in subjects.iter().enumerate() {
        let trials =
            &results[index * NEGLECT_TRIALS as usize..(index + 1) * NEGLECT_TRIALS as usize];
        let mut broken: Vec<&str> = trials.iter().filter_map(|r| r.broken_in).collect();
        let broken_count = broken.len();
        broken.sort_unstable();
        broken.dedup();
        let mean = |pick: fn(&NeglectResult) -> Option<(f32, f32)>| {
            let values: Vec<(f32, f32)> = trials.iter().filter_map(pick).collect();
            if values.is_empty() {
                return "-".to_string();
            }
            let n = values.len() as f32;
            let (d, c) = values
                .iter()
                .fold((0.0, 0.0), |(a, b), (x, y)| (a + x, b + y));
            format!("{:.2}/{:.2}", d / n, c / n)
        };
        let returned_home = trials
            .iter()
            .filter(|r| {
                r.returned
                    .is_some_and(|(d, c)| d < RETURNED_DISTANCE && c < RETURNED_COLOUR)
            })
            .count();
        println!(
            "  {:28} 壊れた {broken_count}/{NEGLECT_TRIALS}{} | 弱りきったとき {} | 戻した後 {} | 戻った {returned_home}/{NEGLECT_TRIALS}",
            subject.label,
            if broken.is_empty() {
                String::new()
            } else {
                format!("({})", broken.join("・"))
            },
            mean(|r| r.neglected),
            mean(|r| r.returned),
        );
    }
    println!("合計 {:.0} 秒", started.elapsed().as_secs_f32());
}

fn main() {
    let mut args = std::env::args().skip(1);
    let directory = PathBuf::from(args.next().expect("found の JSON があるディレクトリを渡す"));
    let second = args.next().expect("出力先か --trace を渡す");
    if second == "--trace" {
        let file = args
            .next()
            .expect("--trace の後にファイル名(221 など)を渡す");
        let index = args
            .next()
            .and_then(|i| i.parse().ok())
            .expect("--trace の後に生物の番号を渡す");
        trace(&directory, &file, index);
        return;
    }
    if second == "--suite" {
        let output = PathBuf::from(args.next().expect("--suite の後に出力先を渡す"));
        suite(&directory, &output);
        return;
    }
    if second == "--neglect" {
        let output = PathBuf::from(args.next().expect("--neglect の後に出力先を渡す"));
        neglect(&output);
        return;
    }
    let output = PathBuf::from(second);
    fs::create_dir_all(&output).expect("出力先を作れない");
    let started = Instant::now();

    let animals: Vec<(String, usize, AnimalData)> = FILES
        .iter()
        .flat_map(|name| {
            load(&directory, name)
                .into_iter()
                .enumerate()
                .map(move |(index, animal)| (name.to_string(), index, animal))
        })
        .collect();
    for (_, _, animal) in &animals {
        for kernel in &animal.params {
            assert_eq!(
                (kernel.kn, kernel.gn),
                (1, 1),
                "多項式以外の形は扱っていない"
            );
        }
    }
    let jobs: Vec<(usize, usize)> = (0..animals.len())
        .flat_map(|a| (0..VARIANTS.len()).map(move |v| (a, v)))
        .collect();
    let job_count = jobs.len();
    let queue = Mutex::new(jobs.into_iter().enumerate());
    let results: Mutex<Vec<Option<Outcome>>> = Mutex::new((0..job_count).map(|_| None).collect());
    let workers = thread::available_parallelism().map_or(4, |n| n.get());
    thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let Some((index, (animal_index, variant))) = queue.lock().unwrap().next() else {
                    break;
                };
                let (file, number, animal) = &animals[animal_index];
                let (_, size, shrink) = VARIANTS[variant];
                let image = output.join(format!("{file}_{number:02}_v{variant}.ppm"));
                let outcome = run(animal, size, shrink, &image);
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

    println!(
        "{STEPS} ステップ。総量×は置いたときとの比、[] はチャンネルごとの比、塊は {CHECKPOINT} ステップ目→最後の主な塊の数、速さは最後の {SPEED_WINDOW} ステップ"
    );
    for (animal_index, (file, number, animal)) in animals.iter().enumerate() {
        let p0 = &animal.params[0];
        println!(
            "{file} #{number:02} {:10} {:26} R={:>2} カーネル {:>2}・チャンネル {}",
            animal.code,
            animal.name,
            p0.radius,
            animal.params.len(),
            animal.cells.len()
        );
        for (variant, (label, _, _)) in VARIANTS.iter().enumerate() {
            let outcome = &results[animal_index * VARIANTS.len() + variant];
            println!("    {label:16} {}", format_outcome(outcome));
        }
    }
    println!();
    for (variant, (label, _, _)) in VARIANTS.iter().enumerate() {
        let outcomes: Vec<&Outcome> = (0..animals.len())
            .map(|a| &results[a * VARIANTS.len() + variant])
            .collect();
        let count = |fate: &str| outcomes.iter().filter(|o| o.fate == fate).count();
        let moving = outcomes
            .iter()
            .filter(|o| o.fate == "生存" && o.speed > 0.1)
            .count();
        let single = outcomes
            .iter()
            .filter(|o| o.fate == "生存" && o.blobs[1] == 1)
            .count();
        println!(
            "{label:16} 生存 {} (うち動く {moving}・主な塊が1つ {single}) / 崩壊 {} / 膨張 {} / 置けない {}",
            count("生存"),
            count("崩壊"),
            count("膨張"),
            count("置けない")
        );
    }
    println!("合計 {:.0} 秒", started.elapsed().as_secs_f32());
}
