//! 【実験】Glaberish(Davis & Bongard 2022、実装 yuca は MIT)を取り込み、2つを確かめる
//! (docs/DESIGN.md「Glaberish を取り込む」)。ペット本体の振る舞いには触れない。
//!
//! 1. **今までの生き物はそのまま使えるか**: 生まれる関数と生き残る関数を、その生物の成長関数に
//!    そろえると、式の上ではいまの Lenia と同じ規則になる。浮動小数点の計算順序のぶんだけずれる
//!    ので、全生物をいまの規則と並べて 20000 ステップ動かし、崩壊と、総量が1%以上ずれたステップを
//!    見る
//! 2. **移植が合っているか**: yuca の s613 の規則(`ca_configs/s613.npy`。カーネル半径31のガウス
//!    3本の輪、生まれる関数 μ=0.0621 σ=0.0088、生き残る関数 μ=0.2151 σ=0.0369、Δt=0.1)に、
//!    yuca が見つけたカエルなどのパターン(`yuca/zoo/*.npy` を JSON に書き出したもの)を置き、
//!    形を保って動くかを見る。ランダムな初期状態からも1回動かす
//!
//! 使い方: `cargo run --release -p vmc-pet-body --example glaberish_trial -- <パターンの JSON が
//! あるディレクトリ>`。その下の `glaberish/` にスナップショットを PGM で書き出す。

use std::fs;
use std::path::{Path, PathBuf};
use std::thread;

use serde::Deserialize;
use vmc_pet_body::lenia::{GrowthMapping, KernelCore, LeniaParams};
use vmc_pet_body::{load_animal, Field, GrowthFunction, Lenia};

const COLLAPSE_MASS: f32 = 5.0;

const COMPARE_FIELD: usize = 64;
const COMPARE_STEPS: u32 = 20_000;
/// 2つの規則の総量が、これ以上の割合でずれたら「軌跡が分かれた」とみなす。
const DIVERGED_RATIO: f32 = 0.01;

const S613_FIELD: usize = 128;
const S613_STEPS: u32 = 3_000;
const S613_SNAPSHOTS: [u32; 7] = [0, 100, 250, 500, 1000, 2000, 3000];
/// yuca の「生きている」の目安(`alive_threshold`)。塊を数えるのに使う。
const ALIVE_THRESHOLD: f32 = 0.1;
const PATTERNS: [&str; 3] = ["s613_s613_frog000", "frog000", "s613_fast_wobble_glider000"];

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
}

struct Comparison {
    code: String,
    classic_collapsed_at: Option<u32>,
    glaberish_collapsed_at: Option<u32>,
    classic_mass: f32,
    glaberish_mass: f32,
    diverged_at: Option<u32>,
}

/// 確かめ1: いまの規則と、成長関数を1つにそろえた Glaberish を並べて動かす。
fn compare(code: &str) -> Comparison {
    let animal = load_animal(code).unwrap();
    let growth = animal.params.growth_function();
    let mut classic_lenia = Lenia::new(animal.params.clone());
    let mut glaberish_lenia = Lenia::new(animal.params.clone());
    let mut classic = Field::new(COMPARE_FIELD, COMPARE_FIELD);
    let mut glaberish = Field::new(COMPARE_FIELD, COMPARE_FIELD);
    classic.place_centered(&animal.pattern);
    glaberish.place_centered(&animal.pattern);

    let mut comparison = Comparison {
        code: code.to_string(),
        classic_collapsed_at: None,
        glaberish_collapsed_at: None,
        classic_mass: 0.0,
        glaberish_mass: 0.0,
        diverged_at: None,
    };
    for step in 1..=COMPARE_STEPS {
        classic_lenia.step_at_tempo(&mut classic, 1.0, 1.0);
        glaberish_lenia.step_glaberish(&mut glaberish, growth, growth, 1.0, 1.0);
        let (classic_mass, glaberish_mass) = (classic.mass(), glaberish.mass());
        if classic_mass < COLLAPSE_MASS && comparison.classic_collapsed_at.is_none() {
            comparison.classic_collapsed_at = Some(step);
        }
        if glaberish_mass < COLLAPSE_MASS && comparison.glaberish_collapsed_at.is_none() {
            comparison.glaberish_collapsed_at = Some(step);
        }
        let scale = classic_mass.max(glaberish_mass).max(1e-6);
        if comparison.diverged_at.is_none()
            && (classic_mass - glaberish_mass).abs() / scale > DIVERGED_RATIO
        {
            comparison.diverged_at = Some(step);
        }
        comparison.classic_mass = classic_mass;
        comparison.glaberish_mass = glaberish_mass;
    }
    comparison
}

/// yuca の zoo のパターンを書き出した JSON。
#[derive(Deserialize)]
struct PatternFile {
    rows: usize,
    cols: usize,
    values: Vec<f32>,
}

/// yuca の s613 の規則。カーネルはガウス3本の輪(Hydrogeminium natans と同じ形)。
fn s613() -> (Lenia, GrowthFunction, GrowthFunction) {
    let params = LeniaParams {
        radius: 31,
        time_divisor: 10.0,
        // カーネルは下の輪の形から作るので、この2つは使われない
        kernel_peaks: vec![1.0],
        kernel_core: KernelCore::Polynomial,
        growth_center: 0.0621,
        growth_width: 0.0088,
        growth_mapping: GrowthMapping::Exponential,
    };
    // (高さ, 中心, 幅)。yuca の GaussianMixture は足し合わせた後に 0〜1 に切り詰める
    let rings: [(f32, f32, f32); 3] = [
        (0.5, 0.093809, 0.033),
        (1.0, 0.28143, 0.033),
        (2.0 / 3.0, 0.46904, 0.033),
    ];
    let profile = move |distance: f32| {
        rings
            .iter()
            .map(|(height, center, width)| {
                height * (-((distance - center) / width).powi(2) / 2.0).exp()
            })
            .sum::<f32>()
            .clamp(0.0, 1.0)
    };
    let genesis = GrowthFunction {
        mapping: GrowthMapping::Exponential,
        center: 0.0621,
        width: 0.0088,
    };
    let persistence = GrowthFunction {
        mapping: GrowthMapping::Exponential,
        center: 0.2151,
        width: 0.0369,
    };
    (
        Lenia::with_radial_profile(params, profile),
        genesis,
        persistence,
    )
}

/// 値が `ALIVE_THRESHOLD` を超えるセルがつながった塊の数(トーラス、上下左右)。
fn blob_count(field: &Field) -> usize {
    let view = field.view();
    let (width, height) = (view.width(), view.height());
    let mut seen = vec![false; width * height];
    let mut blobs = 0;
    for start in 0..width * height {
        let alive = |index: usize| view.get(index % width, index / width) > ALIVE_THRESHOLD;
        if seen[start] || !alive(start) {
            continue;
        }
        blobs += 1;
        let mut stack = vec![start];
        seen[start] = true;
        while let Some(index) = stack.pop() {
            let (x, y) = (index % width, index / width);
            let neighbours = [
                ((x + 1) % width, y),
                ((x + width - 1) % width, y),
                (x, (y + 1) % height),
                (x, (y + height - 1) % height),
            ];
            for (nx, ny) in neighbours {
                let neighbour = ny * width + nx;
                if !seen[neighbour] && alive(neighbour) {
                    seen[neighbour] = true;
                    stack.push(neighbour);
                }
            }
        }
    }
    blobs
}

fn write_pgm(path: &Path, field: &Field) {
    let view = field.view();
    let (width, height) = (view.width(), view.height());
    let scale = 2;
    let mut bytes = format!("P5\n{} {}\n255\n", width * scale, height * scale).into_bytes();
    for y in 0..height * scale {
        for x in 0..width * scale {
            let value = view.get(x / scale, y / scale).clamp(0.0, 1.0);
            bytes.push((value * 255.0).round() as u8);
        }
    }
    fs::write(path, bytes).expect("PGM を書き出せない");
}

/// 確かめ2: s613 の規則で、場に置いたものを動かす。
fn run_s613(name: &str, mut field: Field, output: &Path) -> String {
    let (mut lenia, genesis, persistence) = s613();
    let size = S613_FIELD as f32;
    let wrap = |offset: f32| {
        let offset = offset.rem_euclid(size);
        if offset > size / 2.0 {
            offset - size
        } else {
            offset
        }
    };
    let mut line = format!("{name:28}");
    let mut previous = field.view().toroidal_centroid();
    let mut travelled = 0.0f32;
    for step in 0..=S613_STEPS {
        if S613_SNAPSHOTS.contains(&step) {
            line += &format!(
                " | {step:>4}: 総量 {:7.1}・塊 {:2}・道のり {:5.0}",
                field.mass(),
                blob_count(&field),
                travelled
            );
            write_pgm(&output.join(format!("{name}_{step:05}.pgm")), &field);
        }
        if step == S613_STEPS {
            break;
        }
        lenia.step_glaberish(&mut field, genesis, persistence, 1.0, 1.0);
        let current = field.view().toroidal_centroid();
        if let (Some(a), Some(b)) = (previous, current) {
            let (dx, dy) = (wrap(b.0 - a.0), wrap(b.1 - a.1));
            travelled += (dx * dx + dy * dy).sqrt();
        }
        previous = current;
    }
    line
}

fn pattern_field(directory: &Path, name: &str) -> Field {
    let text = fs::read_to_string(directory.join(format!("{name}.json")))
        .unwrap_or_else(|_| panic!("{name}.json を読めない"));
    let pattern: PatternFile = serde_json::from_str(&text).expect("パターンの JSON が壊れている");
    let top = (S613_FIELD - pattern.rows) / 2;
    let left = (S613_FIELD - pattern.cols) / 2;
    let mut field = Field::new(S613_FIELD, S613_FIELD);
    field.map(|x, y, _| {
        let inside =
            (top..top + pattern.rows).contains(&y) && (left..left + pattern.cols).contains(&x);
        if inside {
            pattern.values[(y - top) * pattern.cols + (x - left)]
        } else {
            0.0
        }
    });
    field
}

fn random_field() -> Field {
    let mut rng = Rng(0x5EED_0613_0000_0001);
    let values: Vec<f32> = (0..S613_FIELD * S613_FIELD).map(|_| rng.unit()).collect();
    let mut field = Field::new(S613_FIELD, S613_FIELD);
    field.map(|x, y, _| values[y * S613_FIELD + x]);
    field
}

fn main() {
    let directory = PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("パターンの JSON があるディレクトリを引数で渡す"),
    );
    let output = directory.join("glaberish");
    fs::create_dir_all(&output).expect("出力ディレクトリを作れない");
    let codes: Vec<String> = vmc_pet_body::list_animals()
        .unwrap()
        .into_iter()
        .map(|(code, _)| code)
        .collect();

    let (comparisons, s613_lines) = thread::scope(|scope| {
        let comparison_handles: Vec<_> = codes
            .iter()
            .map(|code| scope.spawn(move || compare(code)))
            .collect();
        let mut s613_handles = Vec::new();
        for name in PATTERNS {
            let (directory, output) = (&directory, &output);
            s613_handles
                .push(scope.spawn(move || run_s613(name, pattern_field(directory, name), output)));
        }
        let output = &output;
        s613_handles.push(scope.spawn(move || run_s613("random", random_field(), output)));
        (
            comparison_handles
                .into_iter()
                .map(|h| h.join().unwrap())
                .collect::<Vec<_>>(),
            s613_handles
                .into_iter()
                .map(|h| h.join().unwrap())
                .collect::<Vec<_>>(),
        )
    });

    println!(
        "==== 確かめ1: いまの規則と、成長関数を1つにそろえた Glaberish({COMPARE_FIELD}×{COMPARE_FIELD}、{COMPARE_STEPS} ステップ) ===="
    );
    for c in &comparisons {
        let collapse =
            |at: Option<u32>| at.map_or("崩壊せず".to_string(), |s| format!("{s} で崩壊"));
        println!(
            "  {:6} いまの規則: {}・総量 {:.2} | Glaberish: {}・総量 {:.2} | 総量が1%以上ずれた: {}",
            c.code,
            collapse(c.classic_collapsed_at),
            c.classic_mass,
            collapse(c.glaberish_collapsed_at),
            c.glaberish_mass,
            c.diverged_at.map_or("ずれず".to_string(), |s| format!("{s} ステップ目"))
        );
    }
    println!();
    println!(
        "==== 確かめ2: s613 の規則({S613_FIELD}×{S613_FIELD}、{S613_STEPS} ステップ。塊は値 {ALIVE_THRESHOLD} 超) ===="
    );
    for line in s613_lines {
        println!("  {line}");
    }
}
