//! 【軽い実験】SmoothLife(Rafler 2011)で、どんな見た目の生き物が現れるかを見る。
//!
//! issue #1 の候補のうち、SmoothLife は文献上「特定の粗さでしか安定しない」グライダーを持つ
//! (arXiv 2401.13111)ため実装を見送っていたが、見た目がいまの Lenia と違って面白そうなので、
//! 一度動かして見る(docs/DESIGN.md「SmoothLife を軽く動かしてみた」)。ペット本体の振る舞いには
//! 触れない。
//!
//! 規則(Rafler 2011): 各セルで、内側の円(半径 ri)の平均 m と、外側の輪(ri〜ra、ra = 3 ri)の
//! 平均 n を求める(縁は幅1で滑らかにする)。遷移関数 s(n, m) は、m に応じて「生まれる区間」と
//! 「生き残る区間」を切り替え、n がその区間にあるかを滑らかに判定する。規則の2つの組は、
//! 参考にした実装(duckythescientist/SmoothLife の `BasicRules` と `SmoothTimestepRules`)と同じ。
//!
//! - `BasicRules`: 区間 [0.278, 0.365] / [0.267, 0.445]、ロジスティックの滑らかな階段で区間の端を
//!   m に応じて混ぜ、離散時間(f ← s)で進める
//! - `SmoothTimestepRules`: 区間 [0.254, 0.312] / [0.340, 0.518]、直線の滑らかな階段で n を判定し、
//!   m > 0.5 で区間を切り替え、目標へ近づける時間の進め方(f ← f + dt(s − f))で進める
//!
//! 場は、参考にした実装と同じく、辺が ra の正方形を「場の面積 ÷ (2 ra)²」個ばらまいて始める
//! (まばらに置くと消えてしまう)。
//!
//! 使い方: `cargo run --release -p vmc-pet-body --example smoothlife_trial -- <出力ディレクトリ>`。
//! 条件ごと・時刻ごとの場を PGM で書き出し、占有率・塊の数・直前10ステップの変化量を表示する。
//! 出力ディレクトリの後ろに `--animate` を付けると、条件 E(グライダー)と F(紐でつながった塊)の
//! 場を細かい間隔で書き出す(アニメーションにするため。スナップショットと同じ初期配置なので、
//! 同じ動きになる)。

use std::fs;
use std::path::{Path, PathBuf};
use std::thread;

/// 区間の端の滑らかさ(n の判定)と、生きているかの判定(m)の幅。
const ALPHA_N: f32 = 0.028;
const ALPHA_M: f32 = 0.147;
/// 円と輪の縁を滑らかにする幅(セル)。
const RIM_WIDTH: f32 = 1.0;
const SNAPSHOT_STEPS: [u32; 7] = [0, 50, 100, 200, 400, 800, 1600];
/// 変化量を見る幅(この前のステップとの差の平均)。
const ACTIVITY_WINDOW: u32 = 10;
/// 書き出す画像の一辺(場の大きさによらずこの大きさに拡大する)。
const IMAGE_SIDE: usize = 256;

#[derive(Debug, Clone, Copy)]
enum Rules {
    /// 離散時間の SmoothLife。
    Basic,
    /// 目標へ近づける時間の進め方の SmoothLife。
    SmoothTimestep { dt: f32 },
}

struct Condition {
    name: &'static str,
    rules: Rules,
    size: usize,
    outer_radius: f32,
}

const CONDITIONS: [Condition; 5] = [
    Condition {
        name: "E_128_ra21_basic",
        rules: Rules::Basic,
        size: 128,
        outer_radius: 21.0,
    },
    Condition {
        name: "F_128_ra21_smooth0.1",
        rules: Rules::SmoothTimestep { dt: 0.1 },
        size: 128,
        outer_radius: 21.0,
    },
    Condition {
        name: "G_64_ra11_smooth0.1",
        rules: Rules::SmoothTimestep { dt: 0.1 },
        size: 64,
        outer_radius: 11.0,
    },
    Condition {
        name: "H_64_ra11_basic",
        rules: Rules::Basic,
        size: 64,
        outer_radius: 11.0,
    },
    Condition {
        name: "I_32_ra6_smooth0.1",
        rules: Rules::SmoothTimestep { dt: 0.1 },
        size: 32,
        outer_radius: 6.0,
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

    fn index(&mut self, len: usize) -> usize {
        (self.next_u64() % len as u64) as usize
    }
}

/// 畳み込みの1タップ(中心からの変位と重み)。
struct Tap {
    dx: i32,
    dy: i32,
    weight: f32,
}

/// 内側の円と外側の輪。重みの合計(面積)で割って平均にする。
struct Kernel {
    disk: Vec<Tap>,
    ring: Vec<Tap>,
    disk_area: f32,
    ring_area: f32,
}

/// 半径 `radius` の円の、距離 `distance` のセルに対する重み(縁は幅 `RIM_WIDTH` で滑らか)。
fn disk_weight(distance: f32, radius: f32) -> f32 {
    let half = RIM_WIDTH / 2.0;
    if distance < radius - half {
        1.0
    } else if distance > radius + half {
        0.0
    } else {
        (radius + half - distance) / RIM_WIDTH
    }
}

fn kernel(outer_radius: f32) -> Kernel {
    let inner_radius = outer_radius / 3.0;
    let reach = (outer_radius + RIM_WIDTH).ceil() as i32;
    let (mut disk, mut ring) = (Vec::new(), Vec::new());
    for dy in -reach..=reach {
        for dx in -reach..=reach {
            let distance = ((dx * dx + dy * dy) as f32).sqrt();
            let inner = disk_weight(distance, inner_radius);
            let outer = disk_weight(distance, outer_radius);
            if inner > 0.0 {
                disk.push(Tap {
                    dx,
                    dy,
                    weight: inner,
                });
            }
            if outer - inner > 0.0 {
                ring.push(Tap {
                    dx,
                    dy,
                    weight: outer - inner,
                });
            }
        }
    }
    let disk_area = disk.iter().map(|t| t.weight).sum();
    let ring_area = ring.iter().map(|t| t.weight).sum();
    Kernel {
        disk,
        ring,
        disk_area,
        ring_area,
    }
}

fn logistic_threshold(x: f32, x0: f32, alpha: f32) -> f32 {
    1.0 / (1.0 + (-(x - x0) * 4.0 / alpha).exp())
}

fn linearized_threshold(x: f32, x0: f32, alpha: f32) -> f32 {
    ((x - x0) / alpha + 0.5).clamp(0.0, 1.0)
}

/// `BasicRules` の遷移: m で区間の端を混ぜ、n が区間にあるかをロジスティックで判定する。
fn basic_transition(n: f32, m: f32) -> f32 {
    let aliveness = logistic_threshold(m, 0.5, ALPHA_M);
    let low = 0.278 + (0.267 - 0.278) * aliveness;
    let high = 0.365 + (0.445 - 0.365) * aliveness;
    logistic_threshold(n, low, ALPHA_N) * (1.0 - logistic_threshold(n, high, ALPHA_N))
}

/// `SmoothTimestepRules` の遷移: 生まれる区間と生き残る区間を n で直線の階段として判定し、
/// m > 0.5 ならあとの方を使う。
fn smooth_timestep_transition(n: f32, m: f32) -> f32 {
    let interval = |a: f32, b: f32| {
        linearized_threshold(n, a, ALPHA_N) * (1.0 - linearized_threshold(n, b, ALPHA_N))
    };
    if m > 0.5 {
        interval(0.340, 0.518)
    } else {
        interval(0.254, 0.312)
    }
}

fn average(field: &[f32], size: usize, x: usize, y: usize, taps: &[Tap], area: f32) -> f32 {
    let size_i = size as i32;
    let total: f32 = taps
        .iter()
        .map(|tap| {
            let tx = (x as i32 + tap.dx).rem_euclid(size_i) as usize;
            let ty = (y as i32 + tap.dy).rem_euclid(size_i) as usize;
            field[ty * size + tx] * tap.weight
        })
        .sum();
    total / area
}

fn step(field: &[f32], size: usize, kernel: &Kernel, rules: Rules) -> Vec<f32> {
    let mut next = vec![0.0f32; size * size];
    for y in 0..size {
        for x in 0..size {
            let m = average(field, size, x, y, &kernel.disk, kernel.disk_area);
            let n = average(field, size, x, y, &kernel.ring, kernel.ring_area);
            let current = field[y * size + x];
            next[y * size + x] = match rules {
                Rules::Basic => basic_transition(n, m),
                Rules::SmoothTimestep { dt } => {
                    let target = smooth_timestep_transition(n, m);
                    (current + dt * (target - current)).clamp(0.0, 1.0)
                }
            };
        }
    }
    next
}

/// 辺が ra の正方形(値1)を「場の面積 ÷ (2 ra)²」個ばらまく(参考にした実装と同じ密度)。
fn seeded_field(size: usize, outer_radius: f32, rng: &mut Rng) -> Vec<f32> {
    let side = outer_radius as usize;
    let count = ((size * size) as f32 / (2.0 * outer_radius).powi(2)).max(1.0) as usize;
    let mut field = vec![0.0f32; size * size];
    for _ in 0..count {
        let (left, top) = (rng.index(size), rng.index(size));
        for dy in 0..side {
            for dx in 0..side {
                field[((top + dy) % size) * size + (left + dx) % size] = 1.0;
            }
        }
    }
    field
}

/// 値 0.5 以上のセルがつながった塊の数(トーラス、上下左右のつながり)。
fn blob_count(field: &[f32], size: usize) -> usize {
    let mut seen = vec![false; size * size];
    let mut blobs = 0;
    for start in 0..size * size {
        if seen[start] || field[start] < 0.5 {
            continue;
        }
        blobs += 1;
        let mut stack = vec![start];
        seen[start] = true;
        while let Some(index) = stack.pop() {
            let (x, y) = (index % size, index / size);
            let neighbours = [
                ((x + 1) % size, y),
                ((x + size - 1) % size, y),
                (x, (y + 1) % size),
                (x, (y + size - 1) % size),
            ];
            for (nx, ny) in neighbours {
                let neighbour = ny * size + nx;
                if !seen[neighbour] && field[neighbour] >= 0.5 {
                    seen[neighbour] = true;
                    stack.push(neighbour);
                }
            }
        }
    }
    blobs
}

/// 場を `IMAGE_SIDE` に拡大して PGM(グレースケール)で書き出す。
fn write_pgm(path: &Path, field: &[f32], size: usize) {
    let scale = (IMAGE_SIDE / size).max(1);
    let side = size * scale;
    let mut bytes = format!("P5\n{side} {side}\n255\n").into_bytes();
    for y in 0..side {
        for x in 0..side {
            let value = field[(y / scale) * size + x / scale];
            bytes.push((value.clamp(0.0, 1.0) * 255.0).round() as u8);
        }
    }
    fs::write(path, bytes).expect("PGM を書き出せない");
}

/// 1つの条件を動かし、時刻ごとの様子を1行にまとめて返す。
fn run(condition: &Condition, output: &Path) -> String {
    let kernel = kernel(condition.outer_radius);
    let mut rng = Rng(0x5EED_1234_ABCD_0001 ^ condition.size as u64);
    let mut field = seeded_field(condition.size, condition.outer_radius, &mut rng);
    let cells = (condition.size * condition.size) as f32;
    let mut line = format!("{:22}", condition.name);
    let last = *SNAPSHOT_STEPS.last().unwrap();
    let mut earlier: Option<Vec<f32>> = None;
    for step_index in 0..=last {
        if SNAPSHOT_STEPS.contains(&(step_index + ACTIVITY_WINDOW)) {
            earlier = Some(field.clone());
        }
        if SNAPSHOT_STEPS.contains(&step_index) {
            let mass: f32 = field.iter().sum();
            let activity = earlier.take().map_or("-".to_string(), |before| {
                let change: f32 = field.iter().zip(&before).map(|(a, b)| (a - b).abs()).sum();
                format!("{:.4}", change / cells)
            });
            line += &format!(
                " | {step_index:>4}: 占有 {:.2}・塊 {:2}・変化 {activity}",
                mass / cells,
                blob_count(&field, condition.size)
            );
            write_pgm(
                &output.join(format!("{}_{step_index:04}.pgm", condition.name)),
                &field,
                condition.size,
            );
        }
        if step_index < last {
            field = step(&field, condition.size, &kernel, condition.rules);
        }
    }
    line
}

/// アニメーションにする範囲。`CONDITIONS` の `condition_index` 番目を、`from`〜`to` ステップの間
/// `every` ステップごとに書き出す。
struct Animation {
    condition_index: usize,
    from: u32,
    to: u32,
    every: u32,
}

const ANIMATIONS: [Animation; 2] = [
    // E: グライダーは100ステップまでに残るので、そこから滑っていく様子
    Animation {
        condition_index: 0,
        from: 100,
        to: 700,
        every: 4,
    },
    // F: 最初から形を変えていく様子
    Animation {
        condition_index: 1,
        from: 0,
        to: 900,
        every: 6,
    },
];

/// 1つの条件を動かしながらコマを書き出し、書き出したコマの数を返す。
fn animate(animation: &Animation, output: &Path) -> (String, usize) {
    let condition = &CONDITIONS[animation.condition_index];
    let kernel = kernel(condition.outer_radius);
    let mut rng = Rng(0x5EED_1234_ABCD_0001 ^ condition.size as u64);
    let mut field = seeded_field(condition.size, condition.outer_radius, &mut rng);
    let mut frames = 0;
    for step_index in 0..=animation.to {
        let in_range = step_index >= animation.from;
        if in_range && (step_index - animation.from).is_multiple_of(animation.every) {
            write_pgm(
                &output.join(format!("{}_frame_{step_index:05}.pgm", condition.name)),
                &field,
                condition.size,
            );
            frames += 1;
        }
        if step_index < animation.to {
            field = step(&field, condition.size, &kernel, condition.rules);
        }
    }
    (condition.name.to_string(), frames)
}

fn main() {
    let output = PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("出力ディレクトリを引数で渡す"),
    );
    fs::create_dir_all(&output).expect("出力ディレクトリを作れない");

    if std::env::args().any(|arg| arg == "--animate") {
        let written: Vec<(String, usize)> = thread::scope(|scope| {
            let handles: Vec<_> = ANIMATIONS
                .iter()
                .map(|animation| {
                    let output = &output;
                    scope.spawn(move || animate(animation, output))
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect()
        });
        for (name, frames) in written {
            println!("{name}: {frames} コマ");
        }
        return;
    }

    let lines: Vec<String> = thread::scope(|scope| {
        let handles: Vec<_> = CONDITIONS
            .iter()
            .map(|condition| {
                let output = &output;
                scope.spawn(move || run(condition, output))
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect()
    });
    for line in lines {
        println!("{line}");
    }
}
