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
//! (場はトーラスなので結果は同じになる)。
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

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread;
use std::time::Instant;

use serde::Deserialize;
use vmc_pet_body::lenia::{GrowthMapping, KernelCore};
use vmc_pet_body::{accumulate_into, load_animal, CellPos, Perturbation};

/// 読むファイル(`found{名前}.json`)。2チャンネル・自己1本、2チャンネル・自己2本、3チャンネル・自己1本。
const FILES: [&str; 3] = ["221", "222", "231"];
const STEPS: u32 = 3_000;
/// 速さを測る区間(最後の 300 ステップ)。
const SPEED_WINDOW: u32 = 300;
/// 途中の様子を記録するステップ。
const CHECKPOINT: u32 = 1_000;
/// 総量(全チャンネルの和)がこれ未満なら崩壊。
const COLLAPSE_MASS: f32 = 5.0;
/// どれかのチャンネルがこの値を超えるセルを、はっきり見える点とする。
const CLEARLY_VISIBLE: f32 = 0.2;
/// 塊を数えるときの目安と、主な塊とみなす総量の割合(`flexible_search.rs` と同じ)。
const BLOB_THRESHOLD: f32 = 0.1;
const MAJOR_BLOB_SHARE: f32 = 0.1;
/// 散布型の畳み込みで寄与元とみなす値。
const NEGLIGIBLE_CELL_VALUE: f32 = 1e-5;
const NEGLIGIBLE_KERNEL_WEIGHT: f32 = 1e-5;
/// 縮めた後の R。いまの体(O2u など)と同じ。
const PET_RADIUS: usize = 13;

/// (説明, 場の一辺, R を縮めるか)
const VARIANTS: [(&str, usize, bool); 3] = [
    ("元の R・128×128", 128, false),
    ("R 13・64×64", 64, true),
    ("R 13・32×32", 32, true),
];

#[derive(Deserialize, Clone)]
struct KernelData {
    #[serde(rename = "R")]
    radius: usize,
    #[serde(rename = "T")]
    time_divisor: f32,
    b: String,
    m: f32,
    s: f32,
    #[serde(default = "one")]
    h: f32,
    #[serde(default = "one")]
    r: f32,
    kn: u32,
    gn: u32,
    c: [usize; 2],
}

fn one() -> f32 {
    1.0
}

#[derive(Deserialize, Clone)]
struct AnimalData {
    #[serde(default)]
    code: String,
    #[serde(default)]
    name: String,
    params: Vec<KernelData>,
    cells: Vec<String>,
}

/// "1/2,1" のようなリングの重みを数に直す。
fn parse_fractions(text: &str) -> Vec<f32> {
    text.split(',')
        .map(|part| match part.split_once('/') {
            Some((numerator, denominator)) => {
                numerator.trim().parse::<f32>().unwrap()
                    / denominator.trim().parse::<f32>().unwrap()
            }
            None => part.trim().parse::<f32>().unwrap(),
        })
        .collect()
}

/// `LeniaNDKC.py` の `ch2val`(0〜255)。
fn cell_value(token: &str) -> f32 {
    let chars: Vec<char> = token.chars().collect();
    let value = match chars.as_slice() {
        ['.'] | ['b'] => 0,
        ['o'] => 255,
        [single] => *single as u32 - 'A' as u32 + 1,
        [prefix, letter] => (*prefix as u32 - 'p' as u32) * 24 + (*letter as u32 - 'A' as u32 + 25),
        _ => panic!("読めない RLE の値: {token}"),
    };
    value as f32 / 255.0
}

/// 2次元の RLE を、行優先の値と (幅, 高さ) に直す(`LeniaNDKC.py` の `rle2cells` と同じ手順)。
/// 回数つきの値はその数だけ繰り返し、回数つきの行区切り `n$` は、今の行の後に空の行を n−1 行足す。
/// 短い行は 0 で埋める。
fn decode_rle(rle: &str) -> (Vec<f32>, usize, usize) {
    let mut rows: Vec<Vec<f32>> = Vec::new();
    let mut row: Vec<f32> = Vec::new();
    let mut count = String::new();
    let mut prefix: Option<char> = None;
    let text = format!("{}$", rle.trim_end_matches('!'));
    for ch in text.chars() {
        if ch.is_ascii_digit() {
            count.push(ch);
            continue;
        }
        if ('p'..='y').contains(&ch) || ch == '@' {
            prefix = Some(ch);
            continue;
        }
        let token: String = prefix
            .take()
            .into_iter()
            .chain(std::iter::once(ch))
            .collect();
        let repeat = count.parse::<usize>().unwrap_or(1);
        count.clear();
        if token == "$" {
            rows.push(std::mem::take(&mut row));
            for _ in 1..repeat {
                rows.push(Vec::new());
            }
        } else {
            let value = cell_value(&token);
            row.extend(std::iter::repeat_n(value, repeat));
        }
    }
    let width = rows.iter().map(Vec::len).max().unwrap_or(0);
    let height = rows.len();
    let mut values = vec![0.0; width * height];
    for (y, row) in rows.iter().enumerate() {
        values[y * width..y * width + row.len()].copy_from_slice(row);
    }
    (values, width, height)
}

/// 最近傍で拡大縮小する(`scipy.ndimage.zoom(order=0)` に倣う)。
fn zoom(values: &[f32], width: usize, height: usize, ratio: f32) -> (Vec<f32>, usize, usize) {
    let new_width = ((width as f32 * ratio).round() as usize).max(1);
    let new_height = ((height as f32 * ratio).round() as usize).max(1);
    let source = |new: usize, old: usize, index: usize| {
        if new <= 1 {
            0
        } else {
            ((index as f32 * (old - 1) as f32 / (new - 1) as f32).round() as usize).min(old - 1)
        }
    };
    let mut out = vec![0.0; new_width * new_height];
    for y in 0..new_height {
        for x in 0..new_width {
            let (sx, sy) = (source(new_width, width, x), source(new_height, height, y));
            out[y * new_width + x] = values[sy * width + sx];
        }
    }
    (out, new_width, new_height)
}

/// 多項式の輪(`kn` = 1)。
fn kernel_core(x: f32) -> f32 {
    (4.0 * x * (1.0 - x)).powi(4)
}

/// 多項式の成長関数(`gn` = 1)。
fn growth(potential: f32, center: f32, width: f32) -> f32 {
    let deviation = potential - center;
    (1.0 - deviation * deviation / (9.0 * width * width))
        .max(0.0)
        .powi(4)
        * 2.0
        - 1.0
}

struct Tap {
    dx: i32,
    dy: i32,
    weight: f32,
}

struct Kernel {
    taps: Vec<Tap>,
    center: f32,
    width: f32,
    h: f32,
    source: usize,
    target: usize,
}

/// `LeniaNDKC.py` の `kernel_shell` と同じ形のカーネルを、合計1に正規化して作る。距離は全体の R
/// (最初のカーネルの R)で割り、相対半径 r の内側だけを使う。
fn build_kernel(data: &KernelData, radius: usize) -> Kernel {
    let rings = parse_fractions(&data.b);
    let ring_count = rings.len() as f32;
    let r = radius as i32;
    let mut taps = Vec::new();
    let mut total = 0.0;
    for dy in -r..=r {
        for dx in -r..=r {
            let distance = ((dx * dx + dy * dy) as f32).sqrt() / radius as f32;
            if distance >= data.r {
                continue;
            }
            let scaled = ring_count * distance / data.r;
            let ring = (scaled.floor() as usize).min(rings.len() - 1);
            let weight = kernel_core((scaled % 1.0).min(1.0)) * rings[ring];
            if weight <= NEGLIGIBLE_KERNEL_WEIGHT {
                continue;
            }
            total += weight;
            taps.push(Tap { dx, dy, weight });
        }
    }
    for tap in &mut taps {
        tap.weight /= total;
    }
    Kernel {
        taps,
        center: data.m,
        width: data.s,
        h: data.h,
        source: data.c[0],
        target: data.c[1],
    }
}

struct World {
    size: usize,
    channels: Vec<Vec<f32>>,
    kernels: Vec<Kernel>,
    time_step: f32,
    potential: Vec<f32>,
    increments: Vec<Vec<f32>>,
}

impl World {
    fn step(&mut self) {
        let scales = vec![1.0; self.channels.len()];
        self.step_with(&scales, 1.0);
    }

    /// 育てるチャンネルごとの成長の強さと、テンポを指定して1ステップ進める。成長の強さは、
    /// いまのペットと同じく正の成長にだけ掛ける。どちらも 1 なら `LeniaNDKC.py` と同じ規則。
    fn step_with(&mut self, growth_scales: &[f32], tempo: f32) {
        let size = self.size;
        for increment in &mut self.increments {
            increment.fill(0.0);
        }
        let mut weights = vec![0.0f32; self.channels.len()];
        for kernel in &self.kernels {
            self.potential.fill(0.0);
            for (index, &value) in self.channels[kernel.source].iter().enumerate() {
                if value <= NEGLIGIBLE_CELL_VALUE {
                    continue;
                }
                let (x, y) = ((index % size) as i32, (index / size) as i32);
                for tap in &kernel.taps {
                    let tx = (x + tap.dx).rem_euclid(size as i32) as usize;
                    let ty = (y + tap.dy).rem_euclid(size as i32) as usize;
                    self.potential[ty * size + tx] += value * tap.weight;
                }
            }
            let increment = &mut self.increments[kernel.target];
            let scale = growth_scales[kernel.target];
            let time_step = self.time_step * tempo;
            for (added, &potential) in increment.iter_mut().zip(&self.potential) {
                let grown = growth(potential, kernel.center, kernel.width);
                let scaled = if grown > 0.0 { grown * scale } else { grown };
                *added += time_step * kernel.h * scaled;
            }
            weights[kernel.target] += kernel.h;
        }
        for (channel, values) in self.channels.iter_mut().enumerate() {
            if weights[channel] <= 0.0 {
                continue;
            }
            for (value, increment) in values.iter_mut().zip(&self.increments[channel]) {
                *value = (*value + increment / weights[channel]).clamp(0.0, 1.0);
            }
        }
    }

    fn total(&self, index: usize) -> f32 {
        self.channels.iter().map(|c| c[index]).sum()
    }

    fn mass(&self) -> f32 {
        (0..self.size * self.size).map(|i| self.total(i)).sum()
    }

    fn channel_masses(&self) -> Vec<f32> {
        self.channels.iter().map(|c| c.iter().sum()).collect()
    }

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

/// 生物を条件に合わせて場に置く。場に収まらない(パターンが場より大きい)ときは `None`。
fn place(animal: &AnimalData, size: usize, shrink: bool) -> Option<World> {
    let original_radius = animal.params[0].radius;
    let radius = if shrink { PET_RADIUS } else { original_radius };
    let ratio = radius as f32 / original_radius as f32;
    let mut channels = Vec::new();
    for rle in &animal.cells {
        let (values, width, height) = decode_rle(rle);
        let (values, width, height) = if shrink {
            zoom(&values, width, height, ratio)
        } else {
            (values, width, height)
        };
        if width > size || height > size {
            return None;
        }
        let (left, top) = ((size - width) / 2, (size - height) / 2);
        let mut field = vec![0.0; size * size];
        for y in 0..height {
            for x in 0..width {
                field[(top + y) * size + left + x] = values[y * width + x];
            }
        }
        channels.push(field);
    }
    Some(build_world(&animal.params, radius, channels, size))
}

/// カーネルのパラメータ(全体の R は `radius`)と、チャンネルごとの場の値から世界を作る。
fn build_world(
    params: &[KernelData],
    radius: usize,
    channels: Vec<Vec<f32>>,
    size: usize,
) -> World {
    let kernels = params.iter().map(|p| build_kernel(p, radius)).collect();
    let channel_count = channels.len();
    World {
        size,
        channels,
        kernels,
        time_step: 1.0 / params[0].time_divisor,
        potential: vec![0.0; size * size],
        increments: vec![vec![0.0; size * size]; channel_count],
    }
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

impl World {
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
