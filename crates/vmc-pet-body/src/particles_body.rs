//! 粒子の体(`particles::ParticleWorld`)を、`LeniaBody` と同じメソッド面で体として使うもの。
//!
//! issue #3 で、Lenia の体では安全な範囲の行動が報酬を動かせないことが実測で分かり、
//! 粒子の体(候補 1091)で学んだコントローラ(はぐれた粒子 1 個以上で誘う・強さ 0.13・
//! 距離は箱いっぱい)が PC 版の preview で効いた。ここでそれを `Pet` の仕組み
//! (エネルギー・慣れ・色素・気分・echo・記憶)に繋ぐ。
//!
//! 「体の状態=表示」は、`Pet` の残り(echo・色素・慣れ・カメラ)が Lenia の
//! `FieldView` を前提とするため、粒子を 1 枚の場にラスタ化して供給することで保つ。
//! 場の解像度はペットと同じ 32×32、物理の箱は 16 セル(表示 zoom 2)。
//! 箱 16 セルでも 1091 が保つこと、連打崩し+誘いが通ることは
//! `examples/box16_trial.rs` で確かめた(issue #4 のスパイク1)。
//!
//! ペット側との対応:
//! - **エネルギー**: `Vitality` をそのまま流用する。energy → 力全体の倍率
//!   `force_scale`(×1.0〜×0.6。探索で見つけた倍率のうち、崩さずに弱らせられた
//!   実測のある範囲。body-candidates.md「力を弱めても必ず戻る」)
//! - **コントローラ**: 誘いはこの体の内部ルールに置く。はぐれた粒子が 1 個以上
//!   あれば、最大の塊の重心へ誘う。実機での崩れは 69〜76 秒 → 1 秒になった
//!   (docs/experiments/controller-learning.md「実際の連打で学ばせ直す」)。
//!   Lenia の `AutonomousController`(形をならす突き)とは別物なので、`Pet` 側で
//!   粒子の体のときはスキップする
//! - **崩壊検知**: 粒子は数が保存されるので、崩壊して置き直す仕組みは存在しない
//!   (step が返す値は常に false)
//! - **クリック → 突き**: `Pet::click` の摂動(セル座標・強さ)を、箱の座標への
//!   `poke` に翻訳する。強さは preview の実験と同じく、クリック(body側)の既定
//!   amount 0.20 が poke の速さ 1.0 になる倍率。慣れで弱まったぶんも同じ係数で弱まる
//! - **気分 → テンポ**: 粒子の体にはテンポ(体の時間の進み方)の実測が無い。
//!   初回は繋がない(`set_tempo` で丸めだけする)。気分は色素とエネルギーだけで見せる

use alloc::vec;

use crate::math::floorf;
use crate::particles::{ParticleParams, ParticleWorld, DEFAULT_FIELD_SIZE};
use crate::vitality::Vitality;
use crate::{CellPos, Field, FieldView, Perturbation};

/// 物理の箱の一辺(セル)。表示の場(32 セル)を zoom 2 で割った大きさ。
pub const BOX_SIZE: f32 = DEFAULT_FIELD_SIZE / 2.0;
/// 表示の場の一辺(セル)。ペットと同じ。
pub const VIEW_SIZE: usize = 32;
/// 表示 zoom。物理の箱の 1 セルが、表示の場のいくつぶんか。
pub const ZOOM: f32 = DEFAULT_FIELD_SIZE / BOX_SIZE;
/// 誘いが出る条件。はぐれた粒子の数(実機の連打で学ばせ直した値)。
pub const LURE_STRAYS: usize = 1;
pub const LURE_STRENGTH: f32 = 0.13;
/// 粒子1個が場のセルへ配る値の重み(preview のラスタ化と同じ)。
const PARTICLE_WEIGHT: f32 = 0.35;
/// クリックの摂動(amount)を poke の速さに伸ばす倍率。preview・particle_trial で
/// 戻れた poke の速さ 1.0 に、クリック(body側)の既定強さ 0.20 を合わせる。
const POKE_IMPULSE_PER_AMOUNT: f32 = 5.0;

/// 粒子の体。
pub struct ParticleBody {
    world: ParticleWorld,
    vitality: Vitality,
    /// 粒子を場へ配った表示用の値(0..=1)。echo・色素・慣れ・カメラ・描画が読む。
    field: Field,
    /// このステップで誘っていたか(コントローラの行動の記録)。echo を灯す基準。
    luring: bool,
}

impl ParticleBody {
    /// 候補 `seed` の粒子の体を、一辺 [`BOX_SIZE`] の箱に置いて作る。表示は [`VIEW_SIZE`]。
    pub fn new(seed: u64) -> Self {
        Self::new_with_placement(seed, 1)
    }

    /// 初期配置の乱数 (`world` 種) を指定して作る。粒子の挙動は配置によっても
    /// 変わるため、試験や比較のときに使う。
    pub fn new_with_placement(seed: u64, placement: u64) -> Self {
        Self {
            world: ParticleWorld::new(ParticleParams::from_seed(seed), BOX_SIZE, placement),
            vitality: Vitality::new(),
            field: Field::new(VIEW_SIZE, VIEW_SIZE),
            luring: false,
        }
    }

    /// 体を1ステップ進める。戻り値は、このステップで崩壊を検知して置き直したか。
    /// 粒子は数が保存されるので常に `false`(`LeniaBody::step` と同じ形の戻り値)。
    ///
    /// エネルギーは力全体の倍率に効く。`LeniaBody::step` と同じ順序で、倍率はこの
    /// ステップの前のエネルギーで決め、エネルギーはステップの後に減らす。
    pub fn step(&mut self) -> bool {
        self.set_vigour(self.vitality.growth_scale());
        self.controller_act();
        self.world.step();
        self.vitality.decay_step();
        self.rasterize();
        false
    }

    /// 体の時間の進み方を変える。粒子の体にはテンポの実測が無いため、値は覚えるだけで
    /// 挙動は変わらない(気分 → テンポは次の実験まで繋がない)。範囲への丸めは
    /// `Vitality::set_tempo` が行う(Lenia の体と同じ)。
    pub fn set_tempo(&mut self, tempo: f32) {
        self.vitality.set_tempo(tempo);
    }

    /// 体の量の目安(Lenia の場の総量に相当)。粒子を場へ配った値の総量。
    /// 粒子系は崩壊しないため、この値が `Pet` の崩壊検知の閾値未満になることはない。
    pub fn mass(&self) -> f32 {
        self.field.mass()
    }

    /// 現在のエネルギー(0.0..=1.0)。
    pub fn energy(&self) -> f32 {
        self.vitality.energy()
    }

    /// 環境ストレス(PC 版では CPU 負荷)に応じて、エネルギーを追加で削る。
    pub fn apply_environmental_stress(&mut self, stress: f32) {
        self.vitality.apply_environmental_stress(stress);
    }

    /// 保存されていたエネルギーを復元する(起動時に一度だけ呼ぶ)。
    pub fn restore_energy(&mut self, energy: f32) {
        self.vitality.restore_energy(energy);
    }

    /// 起動していなかった時間ぶん、エネルギーを減らす。
    pub fn apply_offline_decay(&mut self, seconds_away: f32) {
        self.vitality.apply_offline_decay(seconds_away);
    }

    /// 置き直す。粒子は数が保存されるため、何もしない(`LeniaBody::revive` と同じ
    /// 呼び出し形状を保つための noop)。
    pub fn revive(&mut self) {}

    /// 体に摂動を直接注入する(`BodyPort` 経由の窓口)。クリックと同じ翻訳を通す。
    pub fn inject(&mut self, perturbation: Perturbation) {
        self.disturb(perturbation);
    }

    /// 世話をされたぶんだけエネルギーを回復させる。場には触れない
    /// (`LeniaBody::receive_care` と同じ区別)。
    pub fn receive_care(&mut self, weight: f32) {
        self.vitality.receive_care(weight);
    }

    /// 体へ摂動(=クリックの突き)を渡す。エネルギーは変えない(`LeniaBody::disturb` と同じ区別)。
    /// `Pet::touch` が慣れ(attention)で弱めた後の `amount` を渡してくる。
    pub fn disturb(&mut self, perturbation: Perturbation) {
        let (px, py) = cell_to_world(perturbation.at);
        self.world.poke(
            px,
            py,
            perturbation.radius / ZOOM,
            perturbation.amount * POKE_IMPULSE_PER_AMOUNT,
        );
    }

    /// 体を観る。粒子を場へ配った値(0..=1)。
    pub fn observe(&self) -> FieldView<'_> {
        self.field.view()
    }

    /// 自分の行動(誘い)が見える場所。誘っていなければ `None`。
    /// `Pet` はここに echo の光を当てる(体への効き目は誘い自身が持ち、光は
    /// 描画専用に差し替える。Lenia の自己摂動と同じ扱い)。
    pub fn self_action_echo(&self) -> Option<CellPos> {
        if !self.luring {
            return None;
        }
        let (cx, cy) = self.world.observe().center;
        Some(CellPos {
            x: ((cx * ZOOM + 0.5) as usize).min(VIEW_SIZE - 1),
            y: ((cy * ZOOM + 0.5) as usize).min(VIEW_SIZE - 1),
        })
    }

    /// 誘いコントローラ。はぐれた粒子が 1 個以上で、最大の塊の重心へ誘う。
    /// 1 秒ごとの判定(preview の実装と同じ間隔)ではなく毎ステップ測るが、誘いの
    /// 強さ・届く距離は学んだ値そのまま。届く距離は箱いっぱい(32 セル箱で「届く
    /// 距離 39」=「どこにいても届く」だったのと同じ意味)。
    fn controller_act(&mut self) {
        let observed = self.world.observe();
        if observed.strays >= LURE_STRAYS {
            self.world.lure_radius = BOX_SIZE;
            self.world.lure = Some((observed.center.0, observed.center.1, LURE_STRENGTH));
            self.luring = true;
        } else {
            self.world.lure = None;
            self.luring = false;
        }
    }

    /// エネルギー(0..=1)を、力全体の倍率(×0.6..×1.0)に写す。
    /// ×0.6 まで弱めても必ず戻ることは探索の実測で確かめた。
    fn set_vigour(&mut self, growth_scale: f32) {
        self.world.force_scale = 0.6 + 0.4 * growth_scale.clamp(0.0, 1.0);
    }

    /// 粒子を場の値(0..=1)に配る(preview のラスタ化と同じ手順・重み)。
    fn rasterize(&mut self) {
        let mut cells = vec![0.0f32; VIEW_SIZE * VIEW_SIZE];
        for i in 0..self.world.x.len() {
            let (u, v) = (self.world.x[i] * ZOOM - 0.5, self.world.y[i] * ZOOM - 0.5);
            let (left, top) = (floorf(u) as isize, floorf(v) as isize);
            let (gx, gy) = (u - floorf(u), v - floorf(v));
            for (cell_x, cell_y, share) in [
                (left, top, (1.0 - gx) * (1.0 - gy)),
                (left + 1, top, gx * (1.0 - gy)),
                (left, top + 1, (1.0 - gx) * gy),
                (left + 1, top + 1, gx * gy),
            ] {
                if cell_x < 0
                    || cell_y < 0
                    || cell_x >= VIEW_SIZE as isize
                    || cell_y >= VIEW_SIZE as isize
                {
                    continue;
                }
                let index = cell_y as usize * VIEW_SIZE + cell_x as usize;
                cells[index] = (cells[index] + PARTICLE_WEIGHT * share).min(1.0);
            }
        }
        // Field::map は Fn を要求するが (x, y) を渡してくるので、行優先の番号を
        // x, y から出して静止したバッファを読むだけでよい(書き込みはしない)。
        self.field.map(|x, y, _| cells[y * VIEW_SIZE + x]);
    }
}

fn cell_to_world(at: CellPos) -> (f32, f32) {
    ((at.x as f32 + 0.5) / ZOOM, (at.y as f32 + 0.5) / ZOOM)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vitality::MIN_ENERGY;
    fn body() -> ParticleBody {
        ParticleBody::new(1091)
    }

    #[test]
    fn a_step_leaves_the_particles_in_one_piece() {
        // Arrange
        let mut body = body();

        // Act
        for _ in 0..3000 {
            body.step();
        }

        // Assert: 塊は保たれ(描画の総量が下限を超え)、崩壊の戻り値は常に false
        assert!(body.mass() > 5.0, "got mass {}", body.mass());
    }

    #[test]
    fn a_click_pokes_the_body_without_touching_the_energy() {
        // Arrange: 塊の近くを突く(塊の重心は箱の中央付近から始まる)
        let mut body = body();
        let before = body.energy();

        // Act
        body.disturb(Perturbation {
            at: CellPos { x: 16, y: 16 },
            radius: 4.5,
            amount: 0.20,
        });

        // Assert: エネルギーは変わらない(世話は receive_care だけが行う)
        assert_eq!(body.energy(), before);
    }

    #[test]
    fn care_recovers_the_energy_but_never_touches_the_body() {
        // Arrange
        let mut body = body();
        let mass_before = body.mass();

        // Act
        body.receive_care(1.0);

        // Assert
        assert!(body.energy() > MIN_ENERGY);
        assert_eq!(body.mass(), mass_before, "care must not touch the body");
    }

    #[test]
    fn the_lure_controller_only_acts_when_a_particle_strays() {
        // Arrange: 健常な体では、はぐれた粒子は無い
        let mut body = body();
        body.step();

        // Act / Assert: ひとりで歩いているうちは誘わない
        assert!(body.self_action_echo().is_none());
    }

    /// 連打を再現して、はぐれた粒子がコントローラで戻ることを確かめる。
    /// 配置によっては連打でも粒子がはぐれないため、崩れやすい配置(seed 1)に絞る。
    /// クリックの半径は実機の連打より強い(表示半径 9)。60 個のうち少数がはぐれればよい。
    #[test]
    fn a_poked_body_is_lured_back_by_the_controller() {
        // Arrange: 塊の重心の近くを連打して 1 個以上はぐれさせる
        let mut body = ParticleBody::new_with_placement(1091, 1);
        for _ in 0..1500 {
            body.step();
        }
        for click in 0..16 {
            let centre = body
                .observe()
                .toroidal_centroid()
                .unwrap_or((VIEW_SIZE as f32 / 2.0, VIEW_SIZE as f32 / 2.0));
            let to_index = |v: f32| ((v + 0.5) as usize).min(VIEW_SIZE - 1);
            let angle = click as f32 * 0.7;
            body.disturb(Perturbation {
                at: CellPos {
                    x: to_index(centre.0 + 2.0 * angle.cos()),
                    y: to_index(centre.1 + 2.0 * angle.sin()),
                },
                radius: 9.0,
                amount: 0.20,
            });
            for _ in 0..3 {
                body.step();
            }
        }
        for _ in 0..60 {
            body.step();
        }

        // Act: 誘いが効いて、はぐれが無くなるまで進める
        let mut lured = false;
        for _ in 0..3000 {
            body.step();
            if body.self_action_echo().is_some() {
                lured = true;
            }
        }

        // Assert: 誘いが出て、体がまとまり直す
        assert!(lured, "the controller must lure strays back");
        assert!(body.mass() > 5.0, "got mass {}", body.mass());
    }

    #[test]
    fn energy_decays_over_the_same_span_as_the_lenia_body() {
        // Arrange
        let mut body = body();

        // Act: 150 秒ぶん(15 step/s)放置する
        for _ in 0..(15 * 150 + 5) {
            body.step();
        }

        // Assert: エネルギーは尽きる(Vitality は Lenia と同じ決まり)
        assert_eq!(body.energy(), 0.0);
    }

    #[test]
    fn a_week_away_drains_the_energy_but_never_goes_negative() {
        // Arrange
        let mut body = body();

        // Act
        body.apply_offline_decay(7.0 * 24.0 * 60.0 * 60.0);

        // Assert
        assert_eq!(body.energy(), 0.0);
    }

    #[test]
    fn a_night_away_weakens_the_body_without_draining_it() {
        // Arrange
        let mut body = body();

        // Act
        body.apply_offline_decay(8.0 * 60.0 * 60.0);

        // Assert: はっきり弱るが、尽き切らない(vitality.rs と同じ値)
        assert!(body.energy() > 0.0 && body.energy() < 0.5);
    }
}
