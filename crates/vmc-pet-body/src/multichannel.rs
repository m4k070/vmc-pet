//! 【実験】多チャンネル Lenia(Chan 2020, "Lenia and Expanded Universe")。
//!
//! チャンネルごとに場を持ち、カーネルごとに「どのチャンネルを読んで、どのチャンネルを育てるか」を
//! 決める。式は Chan 氏の `LeniaNDKC.py` の `calc_kernel`・`kernel_shell`・`calc_once` を写したもので、
//! numpy で同じ式を写した参照実装と、チャンネルごとの総量が5〜6桁まで一致することを確かめてある
//! (docs/experiments/rule-candidates.md「多チャンネル Lenia の生物を動かす」)。
//!
//! ペット本体(`Pet`)はまだ使っていない。PC 版の `--preview-multichannel` で、同梱した生物
//! (`assets/multichannel.json`、ペットの試験一式に合格した9体)を見て確かめるためにある。
//! PC 専用の実験なので `std` フィーチャの下に置く。

use serde::Deserialize;

/// 同梱した多チャンネルの生物。出典とライセンスは `assets/NOTICE.md`。
const MULTICHANNEL_JSON: &str = include_str!("../../../assets/multichannel.json");

/// 散布型の畳み込みで寄与元とみなす値と、カーネルに含める重み(`lenia.rs` と同じ)。
const NEGLIGIBLE_CELL_VALUE: f32 = 1e-5;
const NEGLIGIBLE_KERNEL_WEIGHT: f32 = 1e-5;

/// 全チャンネルの総量がこれ未満なら崩壊とみなす(`Pet::step` と同じ判定)。
pub const COLLAPSE_MASS: f32 = 5.0;

/// カーネル1本のパラメータ(`LeniaNDKC.py` の生物データと同じ形)。
#[derive(Debug, Deserialize, Clone)]
pub struct KernelData {
    #[serde(rename = "R")]
    pub radius: usize,
    #[serde(rename = "T")]
    pub time_divisor: f32,
    /// リングの重み("1/2,1" のような分数の並び)。
    pub b: String,
    pub m: f32,
    pub s: f32,
    #[serde(default = "one")]
    pub h: f32,
    /// 相対半径。
    #[serde(default = "one")]
    pub r: f32,
    /// カーネルの形と成長関数の形。どちらも 1(多項式)だけを扱う。
    pub kn: u32,
    pub gn: u32,
    /// [読むチャンネル, 育てるチャンネル]
    pub c: [usize; 2],
}

fn one() -> f32 {
    1.0
}

/// 多チャンネルの生物。
#[derive(Debug, Deserialize, Clone)]
pub struct MultiAnimal {
    /// 同梱データでの呼び名(例 "221-09")。Chan 氏のファイルから直接読んだ生物では空。
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub code: String,
    #[serde(default)]
    pub name: String,
    pub params: Vec<KernelData>,
    /// チャンネルごとのセル配置(Lenia 独自の RLE)。
    pub cells: Vec<String>,
}

#[derive(Deserialize)]
struct Document {
    animals: Vec<MultiAnimal>,
}

/// 同梱した多チャンネルの生物をすべて読む。
pub fn list_multichannel() -> Result<Vec<MultiAnimal>, serde_json::Error> {
    serde_json::from_str::<Document>(MULTICHANNEL_JSON).map(|document| document.animals)
}

/// 同梱した多チャンネルの生物を id で読む。見つからなければ `None`。
pub fn load_multichannel(id: &str) -> Result<Option<MultiAnimal>, serde_json::Error> {
    Ok(list_multichannel()?
        .into_iter()
        .find(|animal| animal.id == id))
}

/// "1/2,1" のようなリングの重みを数に直す。
pub fn parse_fractions(text: &str) -> Vec<f32> {
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

/// `LeniaNDKC.py` の `ch2val`(0〜255)を 0〜1 にしたもの。
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
pub fn decode_rle(rle: &str) -> (Vec<f32>, usize, usize) {
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
            .chain(core::iter::once(ch))
            .collect();
        let repeat = count.parse::<usize>().unwrap_or(1);
        count.clear();
        if token == "$" {
            rows.push(core::mem::take(&mut row));
            for _ in 1..repeat {
                rows.push(Vec::new());
            }
        } else {
            let value = cell_value(&token);
            row.extend(core::iter::repeat_n(value, repeat));
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
pub fn zoom(values: &[f32], width: usize, height: usize, ratio: f32) -> (Vec<f32>, usize, usize) {
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

/// 多チャンネルの場。場はトーラス。
pub struct MultiWorld {
    /// 場の一辺(正方形)。
    pub size: usize,
    /// チャンネルごとの場の値(行優先)。
    pub channels: Vec<Vec<f32>>,
    kernels: Vec<Kernel>,
    time_step: f32,
    potential: Vec<f32>,
    increments: Vec<Vec<f32>>,
}

impl MultiWorld {
    /// カーネルのパラメータ(全体の R は `radius`)と、チャンネルごとの場の値から作る。
    pub fn new(params: &[KernelData], radius: usize, channels: Vec<Vec<f32>>, size: usize) -> Self {
        let kernels = params.iter().map(|p| build_kernel(p, radius)).collect();
        let channel_count = channels.len();
        Self {
            size,
            channels,
            kernels,
            time_step: 1.0 / params[0].time_divisor,
            potential: vec![0.0; size * size],
            increments: vec![vec![0.0; size * size]; channel_count],
        }
    }

    /// 生物を、全体の R を `radius` にして、一辺 `size` の場の中央に置く。R が元と違うときは、
    /// Chan 氏のプログラムの変換に倣ってパターンを最近傍で拡大縮小する。パターンが場に
    /// 収まらなければ `None`。
    pub fn place(animal: &MultiAnimal, size: usize, radius: usize) -> Option<Self> {
        let original_radius = animal.params[0].radius;
        let ratio = radius as f32 / original_radius as f32;
        let mut channels = Vec::new();
        for rle in &animal.cells {
            let (values, width, height) = decode_rle(rle);
            let (values, width, height) = if radius == original_radius {
                (values, width, height)
            } else {
                zoom(&values, width, height, ratio)
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
        Some(Self::new(&animal.params, radius, channels, size))
    }

    /// 1ステップ進める(`LeniaNDKC.py` の `calc_once` と同じ規則)。
    pub fn step(&mut self) {
        let scales = vec![1.0; self.channels.len()];
        self.step_with(&scales, 1.0);
    }

    /// 育てるチャンネルごとの成長の強さと、テンポを指定して1ステップ進める。成長の強さは、
    /// いまのペットと同じく正の成長にだけ掛ける。どちらも 1 なら `LeniaNDKC.py` と同じ規則。
    ///
    /// カーネル k ごとに、読むチャンネル c0 の場を畳み込んで成長関数を通し、育てるチャンネル c1 へ
    /// `dt·h_k·G_k` を足す。チャンネルごとに、足したカーネルの `h_k` の合計で割り、0〜1 に
    /// 切り詰める。Chan 氏の実装は FFT で畳み込むが、ここでは体と同じ疎な散布型で畳み込む。
    pub fn step_with(&mut self, growth_scales: &[f32], tempo: f32) {
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

    /// セル `index` の全チャンネルの和。
    pub fn total(&self, index: usize) -> f32 {
        self.channels.iter().map(|c| c[index]).sum()
    }

    /// 全チャンネルの総量。
    pub fn mass(&self) -> f32 {
        (0..self.size * self.size).map(|i| self.total(i)).sum()
    }

    /// チャンネルごとの総量。
    pub fn channel_masses(&self) -> Vec<f32> {
        self.channels.iter().map(|c| c.iter().sum()).collect()
    }

    /// 崩壊したか(全チャンネルの総量が `COLLAPSE_MASS` 未満)。
    pub fn collapsed(&self) -> bool {
        self.mass() < COLLAPSE_MASS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bundled_animal_loads_and_fits_a_64_field_at_radius_13() {
        // Arrange / Act
        let animals = list_multichannel().unwrap();

        // Assert: 同梱した9体すべてが、PC 版のプレビューと同じ条件で置ける
        assert_eq!(animals.len(), 9);
        for animal in &animals {
            let world = MultiWorld::place(animal, 64, 13);
            assert!(world.is_some(), "{} が 64×64 に置けない", animal.id);
            assert_eq!(world.unwrap().channels.len(), animal.cells.len());
        }
    }

    #[test]
    fn a_counted_row_delimiter_adds_empty_rows_and_short_rows_are_padded() {
        // Arrange: 1行目は値2つ。「2$」は、今の行(ここでは空)を足したうえで空の行を 2−1 行足す
        // (`LeniaNDKC.py` の `rle2cells` と同じ)ので空の行が2つ入り、4行目は値1つ
        let rle = "Ao$2$A!";

        // Act
        let (values, width, height) = decode_rle(rle);

        // Assert
        assert_eq!((width, height), (2, 4));
        let a = 1.0 / 255.0;
        assert_eq!(values, vec![a, 1.0, 0.0, 0.0, 0.0, 0.0, a, 0.0]);
    }

    #[test]
    fn fractions_in_ring_weights_are_parsed() {
        // Arrange / Act / Assert
        assert_eq!(parse_fractions("1/2,1"), vec![0.5, 1.0]);
    }

    #[test]
    fn a_bundled_animal_survives_a_short_run_in_the_preview_setting() {
        // Arrange: stable moving(221-09)を、プレビューと同じ 64×64・R 13 に置く
        let animal = load_multichannel("221-09").unwrap().unwrap();
        let mut world = MultiWorld::place(&animal, 64, 13).unwrap();
        let initial = world.mass();

        // Act
        for _ in 0..300 {
            world.step();
        }

        // Assert: 崩壊も膨張もせず、総量が置いたときから大きく外れない
        let ratio = world.mass() / initial;
        assert!(!world.collapsed());
        assert!((0.8..=1.25).contains(&ratio), "総量の比 {ratio}");
    }
}
