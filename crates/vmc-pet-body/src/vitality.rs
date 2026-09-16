//! 体の外側に持つ少数の状態変数(エネルギーとテンポ)と、それを体へ効かせる決まり。
//!
//! 以前は `LeniaBody` のフィールドと定数だった。多チャンネルの体(`multichannel::MultiBody`、
//! PC 版の `--multichannel`)でも、放置されて弱る・触られると回復する・テンポで気分を見せる、を
//! いまのペットと同じ値で効かせるため、ここに切り出した。値も計算の順序も変えていない。
//!
//! - エネルギーは、成長(自己修復)の強さに効かせて「元気/放置されて弱る」を作る
//! - テンポは、体の時間の進み方として気分を見せる

/// エネルギー(気分状態)の値域。1.0 が最も元気、0.0 は完全に放置された状態。
pub(crate) const MIN_ENERGY: f32 = 0.0;
pub(crate) const MAX_ENERGY: f32 = 1.0;

/// 体を触れる(`inject` が呼ばれる)たびに得られるエネルギー。
pub(crate) const ENERGY_PER_TOUCH: f32 = 0.15;

/// 1ステップごとにエネルギーが減る量。15 step/s なので、
/// 何にも触れられなければ約 150 秒(2分30秒)で 0 まで下がる。
const ENERGY_DECAY_PER_STEP: f32 = 1.0 / (15.0 * 150.0);

/// 環境ストレス(stress=1.0、CPU 負荷が張り付いている状態)が続いたとき、
/// 通常の減衰にどれだけ上乗せするか。1.0 は「常に全力の環境ストレスなら
/// 減衰速度が倍になる(150秒 → 75秒で尽きる)」ことを意味する。
///
/// growth_scale には既に崖から十分離れた下限(MIN_GROWTH_SCALE)があるため、
/// エネルギーがどれだけ速く尽きても、その先で体が崩壊することはない
/// (sustained_maximum_stress_still_never_collapses_the_body 参照)。
/// ストレスは「尽きる速さ」だけを変え、尽きた後の安全性には影響しない。
const STRESS_DECAY_MULTIPLIER: f32 = 1.0;

/// 起動していない間にエネルギーが尽きるまでの時間。
///
/// 停止中も時間は進める方針(ユーザーとの相談で決定。docs/DESIGN.md
/// 「プロセスをまたぐ記憶」参照)だが、実行中と同じ速さ(約150秒で尽きる)で
/// 減らすと、少し席を外しただけで毎回尽き切ってしまう。そこで停止中だけは
/// 「12時間で尽きる」ゆるやかな速さにしてある。一晩(8時間ほど)離れると
/// 0.3 前後まで下がってはっきり弱るが、尽き切ってはいない、という加減。
const OFFLINE_DEPLETION_SECONDS: f32 = 12.0 * 60.0 * 60.0;

/// エネルギーが尽きても、成長(自己修復)の強さがここより弱くはならない下限。
///
/// 正の成長を一様に弱めるだけでは「なだらかに弱る」ようにはならないことを実測で
/// 確かめた(body::lenia::growth_scale_has_a_cliff_rather_than_a_gentle_slope 参照)。
/// Orbium は growth_scale が概ね 0.77 を下回ると、なだらかに衰えるのではなく
/// 150 ステップ以内にほぼ完全崩壊する崖になっている。この下限(0.85)は、そこから
/// 実測の崖ぎりぎりを避けるのに十分な余裕(0.08)を持たせた値であり、放置されても
/// 体が消えてしまうことなく、確実に「少し弱った」状態(総量がおよそ 4% 下がる程度)
/// で安定するようにしてある。
const MIN_GROWTH_SCALE: f32 = 0.85;

/// 表情としてのテンポが取りうる範囲。全生物で、エネルギーが尽きた体に徐々にかけて
/// 20000ステップ崩壊しないことを計測した範囲(×0.5〜×1.6)に留める
/// (docs/DESIGN.md「体側の表情の軸を振り分けた」)。成長の強さと違い、この範囲の
/// 中に崩壊の崖は見つからなかった。
const MIN_TEMPO: f32 = 0.5;
const MAX_TEMPO: f32 = 1.6;

/// エネルギーとテンポ。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vitality {
    energy: f32,
    /// 体の時間の進み方(1.0 がいつもどおり)。
    tempo: f32,
}

impl Default for Vitality {
    fn default() -> Self {
        Self::new()
    }
}

impl Vitality {
    /// エネルギー満タン・いつものテンポで始める。
    pub fn new() -> Self {
        Self {
            energy: MAX_ENERGY,
            tempo: 1.0,
        }
    }

    /// いまのエネルギーで決まる、成長(自己修復)の強さ。`MIN_GROWTH_SCALE` を下限とし、
    /// 実測で見つかった崩壊の崖には触れさせない。
    pub fn growth_scale(&self) -> f32 {
        MIN_GROWTH_SCALE + (1.0 - MIN_GROWTH_SCALE) * self.energy
    }

    /// 体の時間の進み方(1.0 がいつもどおり)。
    pub fn tempo(&self) -> f32 {
        self.tempo
    }

    /// 体の時間の進み方を変える。計測で崩壊しないと確かめた範囲(`MIN_TEMPO`〜`MAX_TEMPO`)に丸める。
    pub fn set_tempo(&mut self, tempo: f32) {
        self.tempo = tempo.clamp(MIN_TEMPO, MAX_TEMPO);
    }

    /// 現在のエネルギー(0.0..=1.0)。
    pub fn energy(&self) -> f32 {
        self.energy
    }

    /// 体が1ステップ進んだぶん、エネルギーを減らす。
    pub fn decay_step(&mut self) {
        self.energy = (self.energy - ENERGY_DECAY_PER_STEP).max(MIN_ENERGY);
    }

    /// 環境ストレス(0.0..=1.0 目安)に応じて、エネルギーを追加で削る。
    pub fn apply_environmental_stress(&mut self, stress: f32) {
        let extra_decay = ENERGY_DECAY_PER_STEP * STRESS_DECAY_MULTIPLIER * stress.clamp(0.0, 1.0);
        self.energy = (self.energy - extra_decay).max(MIN_ENERGY);
    }

    /// エネルギーを満タンに戻す(体を置き直すとき)。
    pub fn refill(&mut self) {
        self.energy = MAX_ENERGY;
    }

    /// 保存されていたエネルギーを復元する。保存ファイルが壊れていても値域の不変条件は守る。
    pub fn restore_energy(&mut self, energy: f32) {
        self.energy = energy.clamp(MIN_ENERGY, MAX_ENERGY);
    }

    /// 起動していなかった時間ぶん、エネルギーを減らす(`OFFLINE_DEPLETION_SECONDS` 参照)。
    pub fn apply_offline_decay(&mut self, seconds_away: f32) {
        if seconds_away <= 0.0 {
            return;
        }
        let decay = seconds_away / OFFLINE_DEPLETION_SECONDS;
        self.energy = (self.energy - decay).max(MIN_ENERGY);
    }

    /// 世話をされたぶんだけエネルギーを回復させる。`weight` は世話としてどれだけ数えるか(0.0..=1.0)。
    pub fn receive_care(&mut self, weight: f32) {
        let gain = ENERGY_PER_TOUCH * weight.clamp(0.0, 1.0);
        self.energy = (self.energy + gain).min(MAX_ENERGY);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn growth_scale_runs_from_the_floor_to_full_strength_with_energy() {
        // Arrange
        let mut vitality = Vitality::new();

        // Act / Assert: 満タンなら 1.0、尽きれば下限
        assert_eq!(vitality.growth_scale(), 1.0);
        vitality.restore_energy(0.0);
        assert_eq!(vitality.growth_scale(), MIN_GROWTH_SCALE);
    }

    #[test]
    fn tempo_is_clamped_to_the_measured_safe_range() {
        // Arrange
        let mut vitality = Vitality::new();

        // Act / Assert
        vitality.set_tempo(0.1);
        assert_eq!(vitality.tempo(), MIN_TEMPO);
        vitality.set_tempo(3.0);
        assert_eq!(vitality.tempo(), MAX_TEMPO);
    }

    #[test]
    fn energy_runs_out_after_about_150_seconds_of_steps() {
        // Arrange
        let mut vitality = Vitality::new();

        // Act: 15 step/s で 150 秒ぶん(浮動小数点の誤差ぶん数ステップ余分に)
        for _ in 0..(15 * 150 + 5) {
            vitality.decay_step();
        }

        // Assert
        assert_eq!(vitality.energy(), MIN_ENERGY);
    }
}
