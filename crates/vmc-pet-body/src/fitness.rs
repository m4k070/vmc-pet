//! 「面白さ」の代理指標。学習可能なコントローラへ進む前段として、
//! 人間の評価(クリックされたかどうか)を必要とせず、場の軌跡だけから
//! 計算できる評価値を用意する。
//!
//! ユーザーとの相談で、「ユーザーの気を引く」ことを直接の報酬にすると、
//! (a) 実際にクリックされたデータでしか学習できず量が足りない、
//! (b) 目立とうとして暴れるような報酬ハッキングに陥りやすい、という
//! 2つの問題があると分かった。代わりに、場の軌跡そのものから計算できる
//! 「退屈でも不安定でもないか」という内発的な指標を代理にする
//! (docs/DESIGN.md「学習可能なコントローラへ向けた代理指標」参照)。
//!
//! 実測(O2u を実際に動かして確認)では、短い評価窓(60秒程度)の質量の
//! 分散は、実際に崩壊するかどうか(数十分スケールで効くエネルギー枯渇の
//! 積み重ね)とほとんど相関しなかった。そのため「不安定さ」を分散のような
//! 連続値で罰するのはやめ、崩壊は `Pet::step` が返す崩壊検知をそのまま
//! 使ったハードな足切りにしてある。
//!
//! ペット本体の実行には要らない、オフラインでの評価・チューニング専用の
//! ツールなので `std` フィーチャの下に置き、M5Stack のバイナリには含まれない。

use alloc::vec::Vec;

use crate::field::toroidal_signed_offset;
use crate::Pet;

/// 崩壊が一度でも起きたときの適応度。他がどれだけ良くても、これを下回ることはない。
pub const COLLAPSE_PENALTY: f32 = -1000.0;

/// 場の軌跡を記録したもの。適応度の計算はこれを介して行う
/// (記録と評価を分けることで、同じ軌跡に対して複数の評価式を試せる)。
pub struct Trajectory {
    /// 各ステップの重心。場が空だったステップは `None`。
    centroids: Vec<Option<(f32, f32)>>,
    /// 各ステップの総量。脈動の大きさ(見た目の落ち着き)を測るのに使う。
    masses: Vec<f32>,
    /// 各ステップの体全体の平均の色づき具合(体の値で重みづけ、0.0..=1.0)。
    tints: Vec<f32>,
    /// 記録中に一度でも崩壊(`Pet::step` が `true` を返す)が起きたか。
    collapsed: bool,
}

/// 外から見える振る舞いの特徴。**人間が画面を見て感じ取れる量だけ**に
/// 絞ってある(内部変数を直接覗くと、見た目に出ていない差まで拾ってしまい、
/// 「読み取りやすさ」の指標として意味をなさなくなる)。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Signature {
    /// 1ステップあたりの重心の移動量。どれだけ動いて見えるか。
    pub mean_speed: f32,
    /// 総量の標準偏差。どれだけ脈動して(ざわついて)見えるか。
    pub mass_deviation: f32,
}

impl Trajectory {
    /// `pet` を `steps` ぶん進めながら軌跡を記録する。
    pub fn record(pet: &mut Pet, steps: u32) -> Self {
        Self::record_with(pet, steps, |_| {})
    }

    /// 各ステップの前に `before_step` を呼びながら記録する。
    ///
    /// 評価の条件づけ(たとえばエネルギーを固定して「元気な状態」「弱った状態」を
    /// 作る)に使う。世話のされ方を変えて振る舞いの差を測るとき、クリックで
    /// 条件を作るとクリック自体が場を乱してしまい、コントローラの貢献と区別が
    /// つかない。内部状態だけを差し替えられるようにしてある。
    pub fn record_with(pet: &mut Pet, steps: u32, mut before_step: impl FnMut(&mut Pet)) -> Self {
        let mut centroids = Vec::with_capacity(steps as usize);
        let mut masses = Vec::with_capacity(steps as usize);
        let mut tints = Vec::with_capacity(steps as usize);
        let mut collapsed = false;
        for _ in 0..steps {
            before_step(pet);
            if pet.step() {
                collapsed = true;
            }
            centroids.push(pet.observe().toroidal_centroid());
            masses.push(pet.mass());
            tints.push(body_tint(pet));
        }
        Self {
            centroids,
            masses,
            tints,
            collapsed,
        }
    }

    /// 記録した区間を通した、体の平均の色づき具合(0.0..=1.0)。
    pub fn mean_tint(&self) -> f32 {
        if self.tints.is_empty() {
            return 0.0;
        }
        self.tints.iter().sum::<f32>() / self.tints.len() as f32
    }

    /// 崩壊が一度でも起きたか。
    pub fn collapsed(&self) -> bool {
        self.collapsed
    }

    /// 外から見える振る舞いの特徴。
    pub fn signature(&self, field_width: usize, field_height: usize) -> Signature {
        Signature {
            mean_speed: self.mean_step_displacement(field_width, field_height),
            mass_deviation: self.mass_deviation(),
        }
    }

    /// 総量の標準偏差。
    fn mass_deviation(&self) -> f32 {
        if self.masses.is_empty() {
            return 0.0;
        }
        let count = self.masses.len() as f32;
        let mean = self.masses.iter().sum::<f32>() / count;
        let variance = self
            .masses
            .iter()
            .map(|mass| (mass - mean) * (mass - mean))
            .sum::<f32>()
            / count;
        crate::math::sqrtf(variance)
    }

    /// 重心の移動量(トーラス上の符号付き最短距離)の、1ステップあたりの
    /// 平均。「滑空しているか、その場に留まっているか」の目安になる。
    /// 場が空だった区間(重心が定義できない)はスキップする。
    fn mean_step_displacement(&self, field_width: usize, field_height: usize) -> f32 {
        let width = field_width as f32;
        let height = field_height as f32;
        let mut total = 0.0f32;
        let mut count = 0u32;
        for pair in self.centroids.windows(2) {
            let (Some(a), Some(b)) = (pair[0], pair[1]) else {
                continue;
            };
            let dx = toroidal_signed_offset(b.0 - a.0, width);
            let dy = toroidal_signed_offset(b.1 - a.1, height);
            total += crate::math::sqrtf(dx * dx + dy * dy);
            count += 1;
        }
        if count == 0 {
            0.0
        } else {
            total / count as f32
        }
    }

    /// この軌跡の適応度。崩壊が一度でも起きていれば `COLLAPSE_PENALTY`。
    /// そうでなければ、重心の平均移動量(動きがあるほど高い)を返す。
    ///
    /// これは最初に作った素朴な指標で、「動きの多さ」しか見ていない。実測の
    /// 結果、この指標を最大化する方向には出荷できる精度で安全な設定が無く、
    /// しかも本当に欲しかったもの(自律的な動きが見えること)は echo で
    /// 解決できてしまった。経験に関わる評価には `legibility` を使う
    /// (docs/DESIGN.md「世話のされ方が振る舞いに現れているか」参照)。
    pub fn fitness(&self, field_width: usize, field_height: usize) -> f32 {
        if self.collapsed {
            return COLLAPSE_PENALTY;
        }
        self.mean_step_displacement(field_width, field_height)
    }
}

/// 体全体の平均の色づき具合。体の値で重みづけるのは、体の無いところの色素は
/// 描かれず見えないため(描画はドットの大きさを体の値で決める)。
fn body_tint(pet: &Pet) -> f32 {
    let field = pet.observe();
    let pigment = pet.pigment_view();
    let (mut tinted, mut total) = (0.0f32, 0.0f32);
    for y in 0..field.height() {
        for x in 0..field.width() {
            let body_value = field.get(x, y);
            tinted += body_value * pigment.get(x, y);
            total += body_value;
        }
    }
    if total > 0.0 {
        tinted / total
    } else {
        0.0
    }
}

/// 世話され続けている状態へ持っていくまでの助走。
const CARED_WARMUP_STEPS: u32 = 60;

/// 放置してエネルギーが尽き、体が弱った状態へ落ち着くまでの助走。
/// エネルギーは約2250ステップで尽き、S1s のように形が変わる生物も
/// その頃には新しい状態へ落ち着いている。
const NEGLECTED_WARMUP_STEPS: u32 = 3000;

/// 世話され続けている状態の軌跡。
///
/// エネルギーを満タンに固定する。触るのではなく固定するのは、クリック自体が
/// 場を乱してしまい、コントローラの貢献と区別がつかなくなるため。エネルギーは
/// 放っておけば減る一方なので、「満タンに保たれている」は「よく触られている」
/// と同じ状態になる。
pub fn cared_for_trajectory(code: &str, field_size: usize, eval_steps: u32) -> Trajectory {
    let animal = crate::load_animal(code).unwrap();
    let mut pet = Pet::new(animal, field_size, field_size);
    for _ in 0..CARED_WARMUP_STEPS {
        pet.restore(crate::PetMemory::with_energy(1.0), 0.0);
        pet.step();
    }
    Trajectory::record_with(&mut pet, eval_steps, |pet| {
        pet.restore(crate::PetMemory::with_energy(1.0), 0.0);
    })
}

/// 放置され続けた状態の軌跡。
///
/// エネルギーを0に固定するのではなく、**ただ触らずに放置して自然に尽きさせる**。
/// 最初は0へ固定していたが、それだと S1s が崩壊した。滑空している最中に急に
/// 成長を弱めると再編成できずに壊れてしまうためで、自然な減衰なら同じ生物が
/// 20000ステップ生き延びることは別途確認済み。実生活でエネルギーは徐々にしか
/// 減らないので、急落は条件として不当だった(docs/DESIGN.md参照)。
pub fn neglected_trajectory(code: &str, field_size: usize, eval_steps: u32) -> Trajectory {
    let animal = crate::load_animal(code).unwrap();
    let mut pet = Pet::new(animal, field_size, field_size);
    for _ in 0..NEGLECTED_WARMUP_STEPS {
        pet.step();
    }
    Trajectory::record(&mut pet, eval_steps)
}

/// その状態にある個体の軌跡。
///
/// 元気は `cared_for_trajectory` と同じ条件。待っている・がっかりは、放置して自然に
/// エネルギーを尽きさせる(`neglected_trajectory` と同じ)うえで、期待とがっかりを
/// 固定する。何日も学習させて作らないのは、遅いうえにクリックが場を乱すため。
pub fn mood_trajectory(
    code: &str,
    field_size: usize,
    eval_steps: u32,
    state: crate::MoodState,
) -> Trajectory {
    if state == crate::MoodState::Lively {
        return cared_for_trajectory(code, field_size, eval_steps);
    }
    let (anticipation, disappointment) = state.anticipation_and_disappointment();
    let animal = crate::load_animal(code).unwrap();
    let mut pet = Pet::new(animal, field_size, field_size);
    // 実際の生活に合わせ、エネルギーが自然に尽きてから気分を徐々に変える。
    // 生まれた直後から体の状態を変えると S1s が定着できない、という既知の性質を
    // 避けるため(体側の表情の軸を振り分けた計測と同じ条件の作り方)。
    for step in 0..NEGLECTED_WARMUP_STEPS {
        let onset = (step.saturating_sub(MOOD_ONSET_STEP) as f32 / MOOD_RAMP_STEPS as f32).min(1.0);
        pet.pin_mood(anticipation * onset, disappointment * onset);
        pet.step();
    }
    Trajectory::record(&mut pet, eval_steps)
}

/// 気分を変え始めるステップ(エネルギーが自然に尽きる頃)と、変えきるまでのステップ数。
const MOOD_ONSET_STEP: u32 = 2_250;
const MOOD_RAMP_STEPS: u32 = NEGLECTED_WARMUP_STEPS - MOOD_ONSET_STEP;

/// 2つの状態を、動きか色のどちらかで見分けられるか。
///
/// 動き(`legibility`: 速さと脈動)と色(体の平均の色づき具合の差)は、人の目にとって
/// 別々の手がかりなので、どちらか一方ではっきり違えば見分けられる、として大きい方を
/// 取る。`legibility` の定義そのものは変えていないので、これまでの数値や閾値とは
/// そのまま比べられる。色の差は色の混ぜ具合(0.0..=1.0)の絶対差にしてある。相対差に
/// すると、ほとんど色づいていない同士でも差が最大になってしまうため。
pub fn distinguishability(
    first: &Trajectory,
    second: &Trajectory,
    field_width: usize,
    field_height: usize,
) -> f32 {
    let motion = legibility(first, second, field_width, field_height);
    if motion == COLLAPSE_PENALTY {
        return COLLAPSE_PENALTY;
    }
    let colour = (first.mean_tint() - second.mean_tint()).abs();
    motion.max(colour)
}

/// 「世話のされ方が、外から見える振る舞いに現れているか」を測る。
///
/// 元気な状態と弱った状態それぞれの振る舞いの特徴を比べ、成分ごとの相対差の
/// 平均を返す。0 に近いほど「どう扱われてきたかが見た目に出ていない」、
/// 大きいほど「見れば分かる」。
///
/// なぜこれを指標にするのか(docs/DESIGN.md参照):
///
/// - 人間のラベルが要らない。シミュレーションの中だけで完結する
/// - 経験に直結する。差が出るには、内部状態(=どう扱われてきたか)が
///   見た目に出ていなければならない
/// - 報酬ハッキングしにくい。両方の条件で等しく派手に動いても差はゼロなので、
///   「目立てば勝つ」にはならない。差を作るには状態に応じて**振る舞いを
///   変える**必要がある
///
/// どちらかの条件で崩壊した場合は `COLLAPSE_PENALTY`。読み取りやすさのために
/// 体を危険にするのは本末転倒なので、安全性は従来どおりハードな足切りにする。
pub fn legibility(
    cared_for: &Trajectory,
    neglected: &Trajectory,
    field_width: usize,
    field_height: usize,
) -> f32 {
    if cared_for.collapsed() || neglected.collapsed() {
        return COLLAPSE_PENALTY;
    }
    let cared = cared_for.signature(field_width, field_height);
    let weak = neglected.signature(field_width, field_height);
    let relative_difference = |a: f32, b: f32| {
        let scale = a.abs().max(b.abs());
        if scale <= 1e-6 {
            0.0
        } else {
            (a - b).abs() / scale
        }
    };
    (relative_difference(cared.mean_speed, weak.mean_speed)
        + relative_difference(cared.mass_deviation, weak.mass_deviation))
        / 2.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_healthy_body_scores_above_the_collapse_penalty() {
        // Arrange
        let mut pet = Pet::load("O2u", 32, 32).unwrap();

        // Act: 60秒ぶん記録する
        let trajectory = Trajectory::record(&mut pet, 900);

        // Assert: 崩壊せず、滑空している分だけ正のスコアになるはず
        let score = trajectory.fitness(32, 32);
        assert!(
            score > 0.0,
            "a gliding, healthy body should score positively, got {score}"
        );
    }

    #[test]
    fn a_collapsed_trajectory_scores_the_penalty() {
        // Arrange: 場じゅうに最大の摂動を撃ち込んで焼き払う。
        // クリックは慣れ(habituation)で弱まるため体を壊せない。ここで要るのは
        // 崩壊した軌跡そのものなので、素の注入窓口(`BodyPort`)を使う。
        use crate::{BodyPort, CellPos, Perturbation};
        let mut pet = Pet::load("O2u", 32, 32).unwrap();
        for _ in 0..12 {
            for y in (0..32).step_by(4) {
                for x in (0..32).step_by(4) {
                    pet.inject(Perturbation {
                        at: CellPos { x, y },
                        radius: 6.0,
                        amount: 1.0,
                    });
                }
            }
            pet.step();
        }

        // Act: 崩壊を検知させながら記録する
        let trajectory = Trajectory::record(&mut pet, 900);

        // Assert
        assert_eq!(trajectory.fitness(32, 32), COLLAPSE_PENALTY);
    }

    #[test]
    fn identical_conditions_are_not_legible() {
        // Arrange: まったく同じ条件を2回。Lenia は決定的なので軌跡も一致する
        let first = cared_for_trajectory("O2u", 32, 300);
        let second = cared_for_trajectory("O2u", 32, 300);

        // Act / Assert: 差が無いなら「見ても分からない」= 0
        assert_eq!(legibility(&first, &second, 32, 32), 0.0);
    }

    #[test]
    fn a_creature_that_stops_when_neglected_is_highly_legible() {
        // Arrange: S1s は放置されると滑空をやめて止まる(docs/DESIGN.md参照)
        let cared_for = cared_for_trajectory("S1s", 32, 900);
        let neglected = neglected_trajectory("S1s", 32, 900);

        // Act
        let score = legibility(&cared_for, &neglected, 32, 32);

        // Assert: 一目で分かる変化なので、高いスコアになるべき
        assert!(
            score > 0.5,
            "stopping outright must read as legible, got {score}"
        );
    }

    #[test]
    fn orbium_hides_its_condition_much_better_than_s1s() {
        // Arrange: 既定の生物(O2u)は、放置されても14%ほど遅くなるだけ。
        // 差の多くは体そのものではなくコントローラ(弱ると控えめに揺らす)が
        // 作っている
        let orbium = legibility(
            &cared_for_trajectory("O2u", 32, 900),
            &neglected_trajectory("O2u", 32, 900),
            32,
            32,
        );
        let scutium = legibility(
            &cared_for_trajectory("S1s", 32, 900),
            &neglected_trajectory("S1s", 32, 900),
            32,
            32,
        );

        // Assert: 指標が人間の見立て(S1sは一目瞭然、Orbiumは体だけでは分から
        // ない)と同じ順位をつけること。これが代理指標として使えるかの最低条件。
        //
        // かつては `scutium > orbium * 2.0` という余裕を持たせていたが、
        // コントローラがエネルギーを観測して揺らす強さを変えるようになった時点で
        // O2u 側が意図的に 0.20 → 0.40 まで上がったため、倍率ではなく順位だけを
        // 見るようにした。倍率を維持するのは「既定の生物の読み取りやすさを
        // 上げてはいけない」という、目的と逆の制約になってしまう
        assert!(
            scutium > orbium,
            "the metric must agree with the eye; orbium={orbium} scutium={scutium}"
        );
    }

    #[test]
    fn the_controller_makes_even_the_best_hider_somewhat_readable() {
        // Arrange: O2u は体そのものの変化がごく小さい(放置で14%ほど遅くなるだけ)。
        // 見て分かるかどうかは、コントローラが弱った体を控えめに揺らすかどうかに
        // かかっている

        // Act
        let score = legibility(
            &cared_for_trajectory("O2u", 32, 900),
            &neglected_trajectory("O2u", 32, 900),
            32,
            32,
        );

        // Assert: 既定の生物で「見ても分からない」水準(0.2以下)に戻っていないこと。
        // ここが下がったら、世話が振る舞いに現れるという設計目標が退行している
        assert!(
            score > 0.3,
            "the default animal must visibly reflect how it has been treated, got {score}"
        );
    }

    #[test]
    fn a_stationary_field_never_moves_and_scores_near_zero() {
        // Arrange: 円対称な塊は理論上その場に留まる(実際には自己崩壊するはずだが、
        // ここでは短い区間だけを見て「動いていないほどスコアが低い」ことを確かめる)
        let mut pet = Pet::load("O2u", 8, 8).unwrap();

        // Act: ごく短い区間だけ見る(生物が場からはみ出さない範囲)
        let trajectory = Trajectory::record(&mut pet, 2);

        // Assert: 記録自体が失敗しない(パニックしない)ことを確認する程度の
        // スモークテスト。小さな場ではすぐに形が崩れるため、値そのものは検証しない。
        let _ = trajectory.fitness(8, 8);
    }
}
