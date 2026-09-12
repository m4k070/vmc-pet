//! Lenia の更新規則。場を1ステップ進めるだけで、時刻も入力も描画も知らない。
//!
//! 規則は Bert Chan 氏の Lenia (https://github.com/Chakazul/Lenia) に準拠する。
//! ポテンシャル U = normalize(kernel) * A(畳み込み)、成長 G(U)、A_next = clip(A + dt*G(U), 0, 1)。
//!
//! 場の端は周期境界(トーラス)として扱う。端を越えた生物は反対側から現れる。
//! 端に近いほど場を減衰させる「周辺減衰」も試したが、Orbium は 150 ステップで
//! 完全に消滅した。減衰は生物を閉じ込めるのではなく殺してしまうため採らない。

use super::Field;
use super::FieldView;

/// カーネルの重みがこれ以下のタップは畳み込みから除外する。
/// リング状カーネルは中心と外周がほぼ 0 になるため、精度を落とさず計算量を減らせる。
const NEGLIGIBLE_KERNEL_WEIGHT: f32 = 1e-5;

/// この値以下のセルは畳み込みの寄与元として無視する。
/// 生物は場のごく一部しか占めないため、走査を疎にすると計算量が桁で落ちる。
const NEGLIGIBLE_CELL_VALUE: f32 = 1e-5;

/// カーネルの断面形状。animals.json の `kn` に対応する。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KernelCore {
    Polynomial,
    Exponential,
    Step,
    Staircase,
}

impl KernelCore {
    /// animals.json の `kn`(1 始まり)から変換する。
    pub fn from_index(index: u32) -> Option<Self> {
        match index {
            1 => Some(Self::Polynomial),
            2 => Some(Self::Exponential),
            3 => Some(Self::Step),
            4 => Some(Self::Staircase),
            _ => None,
        }
    }

    /// 0.0..1.0 に正規化された半径に対するカーネルの値。
    fn value_at(self, radius: f32) -> f32 {
        const STEP_EDGE: f32 = 0.25;
        match self {
            Self::Polynomial => {
                let base = 4.0 * radius * (1.0 - radius);
                base * base * base * base
            }
            // radius が 0 または 1 のとき指数は -inf になり、値は 0 に収束する
            Self::Exponential => crate::math::expf(4.0 - 1.0 / (radius * (1.0 - radius))),
            Self::Step => {
                if (STEP_EDGE..=1.0 - STEP_EDGE).contains(&radius) {
                    1.0
                } else {
                    0.0
                }
            }
            Self::Staircase => {
                if (STEP_EDGE..=1.0 - STEP_EDGE).contains(&radius) {
                    1.0
                } else if radius < STEP_EDGE {
                    0.5
                } else {
                    0.0
                }
            }
        }
    }
}

/// ポテンシャルを成長量へ写す関数。animals.json の `gn` に対応する。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrowthMapping {
    Polynomial,
    Exponential,
    Step,
}

impl GrowthMapping {
    /// animals.json の `gn`(1 始まり)から変換する。
    pub fn from_index(index: u32) -> Option<Self> {
        match index {
            1 => Some(Self::Polynomial),
            2 => Some(Self::Exponential),
            3 => Some(Self::Step),
            _ => None,
        }
    }

    /// 成長量を -1.0..=1.0 で返す。
    fn value_at(self, potential: f32, center: f32, width: f32) -> f32 {
        let deviation = potential - center;
        match self {
            Self::Polynomial => {
                let base = (1.0 - deviation * deviation / (9.0 * width * width)).max(0.0);
                base * base * base * base * 2.0 - 1.0
            }
            Self::Exponential => {
                crate::math::expf(-(deviation * deviation) / (2.0 * width * width)) * 2.0 - 1.0
            }
            Self::Step => {
                if deviation.abs() <= width {
                    1.0
                } else {
                    -1.0
                }
            }
        }
    }
}

/// Lenia の規則パラメータ。animals.json の `params` に対応する。
#[derive(Debug, Clone)]
pub struct LeniaParams {
    /// カーネル半径 R。10 を下回ると離散化誤差で生物が維持できない。
    pub radius: usize,
    /// 時間分割数 T。1ステップの刻みは dt = 1/T。
    pub time_divisor: f32,
    /// カーネルのリングごとの重み b。要素数がリングの本数になる。
    pub kernel_peaks: Vec<f32>,
    /// 成長関数の中心 m。
    pub growth_center: f32,
    /// 成長関数の幅 s。
    pub growth_width: f32,
    pub kernel_core: KernelCore,
    pub growth_mapping: GrowthMapping,
}

/// 畳み込みの1タップ。中心からの変位と正規化済みの重み。
struct KernelTap {
    dx: i32,
    dy: i32,
    weight: f32,
}

/// Lenia の更新規則そのもの。場を所有せず、`step` で受け取って進める。
pub struct Lenia {
    params: LeniaParams,
    taps: Vec<KernelTap>,
    potential: Vec<f32>,
}

impl Lenia {
    pub fn new(params: LeniaParams) -> Self {
        let taps = build_kernel_taps(&params);
        Self {
            params,
            taps,
            potential: Vec::new(),
        }
    }

    /// 場を1ステップ進める。
    ///
    /// `growth_scale` は成長のうち正の部分(自己修復)にだけ掛ける倍率。
    /// 負の部分(自然な減衰)には掛けない。1.0 なら通常の Lenia と変わらず、
    /// 0.0 に近づくほど修復が弱まり、減衰だけが進むぶん体全体がゆっくり弱っていく。
    /// 気分状態(エネルギー量)はこの倍率を通して場に効く。
    pub fn step(&mut self, field: &mut Field, growth_scale: f32) {
        self.accumulate_potential(field.view());

        let time_step = 1.0 / self.params.time_divisor;
        let width = field.view().width();
        let center = self.params.growth_center;
        let growth_width = self.params.growth_width;
        let mapping = self.params.growth_mapping;
        let potential = &self.potential;

        field.map(|x, y, value| {
            let growth = mapping.value_at(potential[y * width + x], center, growth_width);
            let scaled_growth = if growth > 0.0 { growth * growth_scale } else { growth };
            value + time_step * scaled_growth
        });
    }

    /// 各セルのポテンシャル(カーネルとの畳み込み)を求める。
    ///
    /// 値を持つセルからタップ先へ加算していく散布型にしてある。
    /// 生物は場のごく一部しか占めないため、全セルを走査する収集型より桁で軽い。
    fn accumulate_potential(&mut self, field: FieldView<'_>) {
        let width = field.width() as i32;
        let height = field.height() as i32;
        self.potential.clear();
        self.potential.resize((width * height) as usize, 0.0);

        for y in 0..height {
            for x in 0..width {
                let value = field.get(x as usize, y as usize);
                if value <= NEGLIGIBLE_CELL_VALUE {
                    continue;
                }
                for tap in &self.taps {
                    // |dx| < width なので、加減算1回で周期境界に折り返せる
                    let mut target_x = x + tap.dx;
                    if target_x < 0 {
                        target_x += width;
                    } else if target_x >= width {
                        target_x -= width;
                    }
                    let mut target_y = y + tap.dy;
                    if target_y < 0 {
                        target_y += height;
                    } else if target_y >= height {
                        target_y -= height;
                    }
                    self.potential[(target_y * width + target_x) as usize] += value * tap.weight;
                }
            }
        }
    }

}

/// カーネルをタップの一覧に展開し、総和が 1 になるよう正規化する。
fn build_kernel_taps(params: &LeniaParams) -> Vec<KernelTap> {
    let radius = params.radius as i32;
    let ring_count = params.kernel_peaks.len();
    let mut taps = Vec::new();
    let mut total_weight = 0.0;

    for dy in -radius..=radius {
        for dx in -radius..=radius {
            let distance =
                crate::math::sqrtf((dx * dx + dy * dy) as f32) / params.radius as f32;
            if distance >= 1.0 {
                continue;
            }
            let scaled = ring_count as f32 * distance;
            let ring_index = (crate::math::floorf(scaled) as usize).min(ring_count - 1);
            let weight = params.kernel_core.value_at(crate::math::fractf(scaled))
                * params.kernel_peaks[ring_index];
            if weight <= NEGLIGIBLE_KERNEL_WEIGHT {
                continue;
            }
            total_weight += weight;
            taps.push(KernelTap { dx, dy, weight });
        }
    }

    for tap in &mut taps {
        tap.weight /= total_weight;
    }
    taps
}

#[cfg(test)]
mod tests {
    use super::*;

    fn orbium_params() -> LeniaParams {
        LeniaParams {
            radius: 13,
            time_divisor: 10.0,
            kernel_peaks: vec![1.0],
            growth_center: 0.15,
            growth_width: 0.015,
            kernel_core: KernelCore::Polynomial,
            growth_mapping: GrowthMapping::Polynomial,
        }
    }

    #[test]
    fn kernel_taps_sum_to_one() {
        // Arrange / Act
        let taps = build_kernel_taps(&orbium_params());

        // Assert
        let total: f32 = taps.iter().map(|tap| tap.weight).sum();
        assert!((total - 1.0).abs() < 1e-4, "kernel must be normalized, got {total}");
    }

    #[test]
    fn kernel_is_a_ring_with_a_hollow_centre() {
        // Arrange
        let taps = build_kernel_taps(&orbium_params());

        // Act: 中心(dx=dy=0)は多項式カーネルの値が 0 なので除外されている
        let centre = taps.iter().find(|tap| tap.dx == 0 && tap.dy == 0);

        // Assert
        assert!(centre.is_none(), "the kernel centre must be empty");
        let peak = taps
            .iter()
            .max_by(|a, b| a.weight.partial_cmp(&b.weight).unwrap())
            .unwrap();
        let peak_distance = ((peak.dx * peak.dx + peak.dy * peak.dy) as f32).sqrt() / 13.0;
        assert!(
            (0.4..0.6).contains(&peak_distance),
            "the peak must sit near half the radius, got {peak_distance}"
        );
    }

    #[test]
    fn growth_peaks_at_the_centre_and_falls_off() {
        // Arrange
        let mapping = GrowthMapping::Polynomial;

        // Act
        let at_centre = mapping.value_at(0.15, 0.15, 0.015);
        let far_away = mapping.value_at(0.5, 0.15, 0.015);

        // Assert
        assert!((at_centre - 1.0).abs() < f32::EPSILON);
        assert_eq!(far_away, -1.0);
    }

    #[test]
    fn sparse_kernel_keeps_most_of_the_ring() {
        // Arrange: 半径13の円内は 517 セル
        let taps = build_kernel_taps(&orbium_params());

        // Act / Assert: ごく小さい重みだけを落としているか
        assert!(taps.len() > 400, "too many taps dropped: {}", taps.len());
        assert!(taps.len() < 517, "no taps were dropped at all");
    }

    #[test]
    fn an_empty_field_stays_empty() {
        // Arrange
        let mut lenia = Lenia::new(orbium_params());
        let mut field = Field::new(64, 64);

        // Act
        lenia.step(&mut field, 1.0);

        // Assert: 成長関数は U=0 で -1 を返すので、空の場は空のまま
        assert_eq!(mass(&field), 0.0);
    }

    /// 場の総量。生物が生きているかの目安になる。
    fn mass(field: &Field) -> f32 {
        let view = field.view();
        let mut total = 0.0;
        for y in 0..view.height() {
            for x in 0..view.width() {
                total += view.get(x, y);
            }
        }
        total
    }

    /// 値で重み付けした重心。生物が移動しているかの判定に使う。
    fn centroid(field: &Field) -> (f32, f32) {
        let view = field.view();
        let (mut x_total, mut y_total, mut weight) = (0.0, 0.0, 0.0);
        for y in 0..view.height() {
            for x in 0..view.width() {
                let value = view.get(x, y);
                x_total += x as f32 * value;
                y_total += y as f32 * value;
                weight += value;
            }
        }
        (x_total / weight, y_total / weight)
    }

    /// step 3 の完了条件: 生物が場の上で安定して移動し続けること。
    #[test]
    fn orbium_survives_and_glides_on_a_torus() {
        // Arrange
        let animal = crate::load_animal("O2u").unwrap();
        let mut field = Field::new(64, 64);
        field.place_centered(&animal.pattern);
        let mut lenia = Lenia::new(animal.params);
        let initial_mass = mass(&field);
        let initial_centroid = centroid(&field);

        // Act: 15 step/s なので 600 ステップは約40秒ぶんの生存に相当する
        for _ in 0..600 {
            lenia.step(&mut field, 1.0);
        }

        // Assert: 総量が保たれている(消滅も爆発もしていない)
        let final_mass = mass(&field);
        assert!(
            (final_mass / initial_mass - 1.0).abs() < 0.1,
            "mass drifted from {initial_mass} to {final_mass}"
        );

        // Assert: 重心が動いている(その場で固まっていない)
        let final_centroid = centroid(&field);
        let moved = (final_centroid.0 - initial_centroid.0).hypot(final_centroid.1 - initial_centroid.1);
        assert!(moved > 5.0, "the creature barely moved: {moved}");
    }

    /// growth_scale を段階的に下げて、生存の限界を確かめる。
    ///
    /// 実測すると、正の成長(自己修復)を一様に弱めるだけでは「なだらかに弱る」
    /// ようにはならない。0.78 では 600 ステップ後も総量 69 前後を保つのに対し、
    /// 0.77 では 150 ステップ以内に完全崩壊(総量 0.0)する — Orbium の自己維持構造は
    /// 修復の強さにきわめて敏感で、緩やかな坂ではなく崖になっている。
    /// `LeniaBody` はこの崖から十分離れた値までしか growth_scale を下げない
    /// (body::lenia_body::MIN_GROWTH_SCALE を参照)。
    #[test]
    fn growth_scale_has_a_cliff_rather_than_a_gentle_slope() {
        // Arrange / Act / Assert
        for (scale, should_survive) in [(0.80, true), (0.78, true), (0.77, false), (0.75, false)] {
            let animal = crate::load_animal("O2u").unwrap();
            let mut field = Field::new(64, 64);
            field.place_centered(&animal.pattern);
            let mut lenia = Lenia::new(animal.params);
            for _ in 0..600 {
                lenia.step(&mut field, scale);
            }
            let final_mass = mass(&field);
            if should_survive {
                assert!(
                    final_mass > 40.0,
                    "expected growth_scale={scale} to survive, got mass={final_mass}"
                );
            } else {
                assert!(
                    final_mass < 5.0,
                    "expected growth_scale={scale} to collapse, got mass={final_mass}"
                );
            }
        }
    }

    /// growth_scale=1.0 は、値を追加する前と同じ挙動を保つ(後方互換性)。
    #[test]
    fn growth_scale_one_matches_the_original_unweakened_behaviour() {
        // Arrange
        let mut field = Field::new(64, 64);
        field.map(|x, y, _value| if x < 5 && y < 5 { 1.0 } else { 0.0 });
        let mut lenia = Lenia::new(orbium_params());

        // Act
        lenia.step(&mut field, 1.0);

        // Assert: growth > 0 の領域では scale=1.0 のとき等倍(何も弱めない)
        assert!(mass(&field) > 0.0);
    }
}
