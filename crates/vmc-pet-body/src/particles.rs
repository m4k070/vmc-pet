//! 【実験】閉じた場を動き回る、非対称な粒子系の体。
//!
//! Lenia の体では、崩壊の崖・形の変化の一方通行・安全な強さで突いても体を導けないこと、が表現の幅を
//! 狭めていた。粒子の数が保存される粒子系なら、総量が尽きて崩壊することがなく、壁は押し返す力として
//! 書ける。種類の違う粒子のあいだの引き合いを非対称にすると、作用と反作用が釣り合わず、塊が自分で進む
//! (docs/experiments/body-candidates.md「閉じた場を動き回る粒子の体を探す」)。
//!
//! 規則は Particle Life 系(Ventrella の Clusters、Mohr の実装の力の形)に倣う。
//!
//! - 粒子は種類を持つ。距離 r(届く距離 `r_max` で割った値)が `BETA` 未満なら全種類共通の反発
//!   `r/BETA − 1`、`BETA`〜1 なら種類の組ごとの係数 `a[i][j]` の山型の引き合い(負なら避け合う)
//! - 速度は毎ステップ `friction` 倍に減衰し、力 × `force` を足す。壁の手前 `WALL_MARGIN` では内向きに
//!   押し返す。1ステップの移動は `MAX_SPEED` まで
//!
//! ペット本体(`Pet`)はまだ使っていない。`examples/particle_trial.rs` の探索・試験と、PC 版の
//! `--preview-particles` で見て確かめるためにある。

use alloc::vec;
use alloc::vec::Vec;

use crate::math::{cosf, sinf, sqrtf};

/// 探索と試験で使った場の一辺(セル)。ペットの場と同じ。
pub const DEFAULT_FIELD_SIZE: f32 = 32.0;
/// 反発が効く距離(`r_max` に対する割合)。
const BETA: f32 = 0.3;
/// 壁から押し返しはじめる距離(セル)と強さ。
const WALL_MARGIN: f32 = 1.5;
const WALL_FORCE: f32 = 0.3;
/// 1ステップで動ける上限(セル)。数値が発散しないための安全弁。
const MAX_SPEED: f32 = 1.0;
/// 誘いが届く距離(セル)の既定値。
pub const LURE_RADIUS: f32 = 10.0;
/// 置きはじめの円盤の半径(セル)。
const INITIAL_RADIUS: f32 = 4.0;

/// 固定シードの疑似乱数(xorshift64)。探索の候補は、この系列から作ったパラメータとして番号で呼ぶ。
pub struct ParticleRng(u64);

impl ParticleRng {
    pub fn new(seed: u64) -> Self {
        // 0 を避け、近いシードでも系列が離れるように混ぜる
        let mut rng = Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03);
        for _ in 0..8 {
            rng.unit();
        }
        rng
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// 0 以上 1 未満の一様乱数。
    pub fn unit(&mut self) -> f32 {
        (self.next() >> 11) as f32 / (1u64 << 53) as f32
    }

    pub fn range(&mut self, low: f32, high: f32) -> f32 {
        low + (high - low) * self.unit()
    }
}

/// 体の規則のパラメータ。
#[derive(Clone, Debug)]
pub struct ParticleParams {
    pub types: usize,
    pub per_type: usize,
    /// `matrix[i * types + j]` は、種類 i の粒子が種類 j の粒子に引かれる強さ(負なら避ける)。
    pub matrix: Vec<f32>,
    pub r_max: f32,
    pub force: f32,
    pub friction: f32,
}

impl ParticleParams {
    /// 探索の候補 `seed` のパラメータ。
    ///
    /// 値を丸めて書き下さず、番号から作り直すのは、Lenia で値を小数4桁に丸めただけで振る舞いが
    /// 変わった経験から(docs/DESIGN.md「丸めで崩壊が反転した」)。試験したのと同じ値を使う。
    pub fn from_seed(seed: u64) -> Self {
        let mut rng = ParticleRng::new(seed);
        let types = if rng.unit() < 0.5 { 2 } else { 3 };
        let per_type = rng.range(20.0, 40.99) as usize;
        let matrix = (0..types * types).map(|_| rng.range(-1.0, 1.0)).collect();
        Self {
            types,
            per_type,
            matrix,
            r_max: rng.range(4.0, 10.0),
            force: rng.range(0.01, 0.1),
            friction: rng.range(0.6, 0.95),
        }
    }

    pub fn count(&self) -> usize {
        self.types * self.per_type
    }
}

/// 体から読める量。
#[derive(Clone, Copy, Debug, Default)]
pub struct ParticleObservation {
    /// 最大の塊にいる粒子の割合。
    pub cohesion: f32,
    /// 最大の塊の重心。
    pub center: (f32, f32),
    /// 最大の塊の広がり(重心からの距離の二乗平均の平方根)。
    pub spread: f32,
    /// 最大の塊に入っていない粒子の、塊の重心からの距離の平均(セル)。はぐれた粒子が無ければ 0。
    pub stray_distance: f32,
    /// 最大の塊に入っていない粒子の数。
    pub strays: usize,
}

fn squared_distance(dx: f32, dy: f32) -> f32 {
    dx * dx + dy * dy
}

fn absolute(value: f32) -> f32 {
    if value < 0.0 {
        -value
    } else {
        value
    }
}

fn kernel(r: f32, attraction: f32) -> f32 {
    if r < BETA {
        r / BETA - 1.0
    } else if r < 1.0 {
        attraction * (1.0 - absolute(2.0 * r - 1.0 - BETA) / (1.0 - BETA))
    } else {
        0.0
    }
}

/// 粒子の体。
pub struct ParticleWorld {
    pub params: ParticleParams,
    /// 場(箱)の一辺(セル)。
    pub size: f32,
    pub x: Vec<f32>,
    pub y: Vec<f32>,
    pub vx: Vec<f32>,
    pub vy: Vec<f32>,
    pub kind: Vec<usize>,
    fx: Vec<f32>,
    fy: Vec<f32>,
    /// 探した規則そのままの係数。`set_vigour` はここから作り直す。
    base_matrix: Vec<f32>,
    /// 力の強さに掛ける倍率(弱らせるときに使う。1.0 がそのまま)。
    pub force_scale: f32,
    /// 誘い: (x, y, 強さ)。`lure_radius` 以内の粒子をその点へ引く。
    pub lure: Option<(f32, f32, f32)>,
    /// 誘いが届く距離(セル)。既定は `LURE_RADIUS`。
    pub lure_radius: f32,
}

impl ParticleWorld {
    /// 一辺 `size` の箱の中央の半径 4 セルの円盤に、種類を混ぜて置く。`seed` は置き方の乱数。
    pub fn new(params: ParticleParams, size: f32, seed: u64) -> Self {
        let mut rng = ParticleRng::new(seed ^ 0xA5A5_5A5A);
        let n = params.count();
        let (mut x, mut y, mut kind) = (Vec::new(), Vec::new(), Vec::new());
        for i in 0..n {
            let angle = rng.range(0.0, core::f32::consts::TAU);
            let radius = INITIAL_RADIUS * sqrtf(rng.unit());
            x.push(size / 2.0 + radius * cosf(angle));
            y.push(size / 2.0 + radius * sinf(angle));
            kind.push(i % params.types);
        }
        Self {
            base_matrix: params.matrix.clone(),
            force_scale: 1.0,
            lure: None,
            lure_radius: LURE_RADIUS,
            params,
            size,
            x,
            y,
            vx: vec![0.0; n],
            vy: vec![0.0; n],
            kind,
            fx: vec![0.0; n],
            fy: vec![0.0; n],
        }
    }

    /// 体を1ステップ進める(テンポ 1.0)。
    pub fn step(&mut self) {
        self.step_at_tempo(1.0);
    }

    /// テンポ(1 ステップで位置へ反映する割合)を指定して1ステップ進める。
    ///
    /// 速度の更新はそのまま(v ← v × friction + a)、位置への反映だけテンポ倍
    /// (x ← x + v × tempo)。Lenia の「1 ステップで進む時間」(`Lenia::step_at_tempo`)
    /// と同じ意味で、力・摩擦・誘い・壁はすべてそのまま効く。MAX_SPEED は速度
    /// そのものの上限なので、テンポを遅くしても巻き込まれない。速いテンポでは
    /// 1 ステップの移動が上限の tempo 倍まで伸びうる点に注意
    pub fn step_at_tempo(&mut self, tempo: f32) {
        let n = self.x.len();
        let (r_max, types) = (self.params.r_max, self.params.types);
        let r_max_squared = r_max * r_max;
        self.fx.iter_mut().for_each(|f| *f = 0.0);
        self.fy.iter_mut().for_each(|f| *f = 0.0);
        for i in 0..n {
            for j in i + 1..n {
                let dx = self.x[j] - self.x[i];
                let dy = self.y[j] - self.y[i];
                let squared = dx * dx + dy * dy;
                if squared >= r_max_squared || squared < 1e-12 {
                    continue;
                }
                let distance = sqrtf(squared);
                let r = distance / r_max;
                let (ux, uy) = (dx / distance, dy / distance);
                let on_i = kernel(r, self.params.matrix[self.kind[i] * types + self.kind[j]]);
                let on_j = kernel(r, self.params.matrix[self.kind[j] * types + self.kind[i]]);
                self.fx[i] += ux * on_i;
                self.fy[i] += uy * on_i;
                self.fx[j] -= ux * on_j;
                self.fy[j] -= uy * on_j;
            }
        }
        let force = self.params.force * self.force_scale;
        for i in 0..n {
            let mut ax = self.fx[i] * force;
            let mut ay = self.fy[i] * force;
            if let Some((lx, ly, strength)) = self.lure {
                let (dx, dy) = (lx - self.x[i], ly - self.y[i]);
                let distance = sqrtf(dx * dx + dy * dy);
                if distance < self.lure_radius && distance > 1e-3 {
                    ax += strength * dx / distance;
                    ay += strength * dy / distance;
                }
            }
            ax += self.wall_push(self.x[i]);
            ay += self.wall_push(self.y[i]);
            let mut vx = self.vx[i] * self.params.friction + ax;
            let mut vy = self.vy[i] * self.params.friction + ay;
            let speed = sqrtf(vx * vx + vy * vy);
            if speed > MAX_SPEED {
                vx *= MAX_SPEED / speed;
                vy *= MAX_SPEED / speed;
            }
            self.vx[i] = vx;
            self.vy[i] = vy;
            self.x[i] = (self.x[i] + vx * tempo).clamp(0.0, self.size - 1e-3);
            self.y[i] = (self.y[i] + vy * tempo).clamp(0.0, self.size - 1e-3);
        }
    }

    fn wall_push(&self, position: f32) -> f32 {
        if position < WALL_MARGIN {
            WALL_FORCE * (WALL_MARGIN - position) / WALL_MARGIN
        } else if position > self.size - WALL_MARGIN {
            -WALL_FORCE * (position - (self.size - WALL_MARGIN)) / WALL_MARGIN
        } else {
            0.0
        }
    }

    /// 係数の非対称な部分(自分で進む力の源)を `vigour` 倍にする。対称な部分(まとまる力)は変えない。
    /// `a = S + vigour·K`、`S = (A + Aᵀ)/2`、`K = (A − Aᵀ)/2`。
    pub fn set_vigour(&mut self, vigour: f32) {
        let types = self.params.types;
        for i in 0..types {
            for j in 0..types {
                let (a, b) = (
                    self.base_matrix[i * types + j],
                    self.base_matrix[j * types + i],
                );
                self.params.matrix[i * types + j] = (a + b) / 2.0 + vigour * (a - b) / 2.0;
            }
        }
    }

    /// 点 (px, py) から半径 `radius` 以内の粒子を、外向きに `impulse` の速さで弾く。
    pub fn poke(&mut self, px: f32, py: f32, radius: f32, impulse: f32) {
        for i in 0..self.x.len() {
            let (dx, dy) = (self.x[i] - px, self.y[i] - py);
            let distance = sqrtf(dx * dx + dy * dy);
            if distance < radius {
                let (ux, uy) = if distance > 1e-3 {
                    (dx / distance, dy / distance)
                } else {
                    (1.0, 0.0)
                };
                self.vx[i] += ux * impulse;
                self.vy[i] += uy * impulse;
            }
        }
    }

    /// 体から読める量。行動を決めるときと、記録を残すときに使う(`examples/particle_trial.rs` と
    /// PC 版の `--particle-log`)。
    pub fn observe(&self) -> ParticleObservation {
        let members = self.largest_cluster();
        let cohesion = members.len() as f32 / self.x.len() as f32;
        let count = members.len() as f32;
        let cx = members.iter().map(|&i| self.x[i]).sum::<f32>() / count;
        let cy = members.iter().map(|&i| self.y[i]).sum::<f32>() / count;
        let spread = sqrtf(
            members
                .iter()
                .map(|&i| squared_distance(self.x[i] - cx, self.y[i] - cy))
                .sum::<f32>()
                / count,
        );
        let mut in_cluster = vec![false; self.x.len()];
        for &i in &members {
            in_cluster[i] = true;
        }
        let strays: Vec<f32> = (0..self.x.len())
            .filter(|&i| !in_cluster[i])
            .map(|i| sqrtf(squared_distance(self.x[i] - cx, self.y[i] - cy)))
            .collect();
        ParticleObservation {
            cohesion,
            center: (cx, cy),
            spread,
            stray_distance: strays.iter().sum::<f32>() / (strays.len().max(1) as f32),
            strays: strays.len(),
        }
    }

    /// 粒子の速さ(セル/ステップ)の平均。
    pub fn mean_speed(&self) -> f32 {
        self.vx
            .iter()
            .zip(&self.vy)
            .map(|(vx, vy)| sqrtf(vx * vx + vy * vy))
            .sum::<f32>()
            / self.x.len() as f32
    }

    /// 最大の塊(`r_max` 以内でつながった粒子の集まり)の添字。
    pub fn largest_cluster(&self) -> Vec<usize> {
        let n = self.x.len();
        let mut parent: Vec<usize> = (0..n).collect();
        fn find(parent: &mut [usize], mut i: usize) -> usize {
            while parent[i] != i {
                parent[i] = parent[parent[i]];
                i = parent[i];
            }
            i
        }
        let link = self.params.r_max * self.params.r_max;
        for i in 0..n {
            for j in i + 1..n {
                let (dx, dy) = (self.x[j] - self.x[i], self.y[j] - self.y[i]);
                if dx * dx + dy * dy < link {
                    let (a, b) = (find(&mut parent, i), find(&mut parent, j));
                    parent[a] = b;
                }
            }
        }
        let mut sizes = vec![0usize; n];
        for i in 0..n {
            let root = find(&mut parent, i);
            sizes[root] += 1;
        }
        let biggest = (0..n).max_by_key(|&i| sizes[i]).unwrap_or(0);
        (0..n)
            .filter(|&i| find(&mut parent, i) == biggest)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_seed_gives_the_same_parameters() {
        // Arrange / Act
        let a = ParticleParams::from_seed(1091);
        let b = ParticleParams::from_seed(1091);

        // Assert
        assert_eq!(a.matrix, b.matrix);
        assert_eq!((a.types, a.per_type), (3, 20));
    }

    #[test]
    fn candidate_1091_stays_in_one_piece_inside_the_box() {
        // Arrange: 探索で合格した候補
        let mut world = ParticleWorld::new(ParticleParams::from_seed(1091), DEFAULT_FIELD_SIZE, 1);

        // Act
        for _ in 0..3000 {
            world.step();
        }

        // Assert: 粒子は箱の中にあり、9割以上が1つの塊にまとまっている
        let inside = world.x.iter().zip(&world.y).all(|(&x, &y)| {
            (0.0..DEFAULT_FIELD_SIZE).contains(&x) && (0.0..DEFAULT_FIELD_SIZE).contains(&y)
        });
        assert!(inside);
        assert!(world.largest_cluster().len() as f32 >= 0.9 * world.x.len() as f32);
    }
}
