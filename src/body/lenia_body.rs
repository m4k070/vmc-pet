//! Lenia を体として実装したもの。
//!
//! `BodyPort` の実装をここに閉じることで、体を差し替える実験の足場になる。
//! 別の CA や別の力学で体を作るなら、同じ窓口を実装した型を用意すればよい。

use super::{Animal, BodyPort, Field, FieldView, Lenia, Perturbation};

/// エネルギー(気分状態)の値域。1.0 が最も元気、0.0 は完全に放置された状態。
const MIN_ENERGY: f32 = 0.0;
const MAX_ENERGY: f32 = 1.0;

/// 体を触れる(`inject` が呼ばれる)たびに得られるエネルギー。
const ENERGY_PER_TOUCH: f32 = 0.15;

/// 1ステップごとにエネルギーが減る量。15 step/s なので、
/// 何にも触れられなければ約 150 秒(2分30秒)で 0 まで下がる。
const ENERGY_DECAY_PER_STEP: f32 = 1.0 / (15.0 * 150.0);

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

/// Lenia の場を体として持つ個体。
pub struct LeniaBody {
    /// 崩壊したときに置き直すため、元の生物を持ち続ける。
    animal: Animal,
    field: Field,
    lenia: Lenia,
    /// 場の外側に持つ少数の状態変数(docs/DESIGN.md「コンセプト」参照)。
    /// 場のパラメータ(成長の強さ)に効かせることで、「元気/放置されて弱る」を作る。
    energy: f32,
}

impl LeniaBody {
    /// 生物を場の中央に配置して体を作る。エネルギーは満タンで始まる。
    pub fn new(animal: Animal, width: usize, height: usize) -> Self {
        let lenia = Lenia::new(animal.params.clone());
        let mut body = Self {
            animal,
            field: Field::new(width, height),
            lenia,
            energy: MAX_ENERGY,
        };
        body.revive();
        body
    }

    /// 体の時間を1ステップ進める。
    /// 外界から呼べる操作ではないため、`BodyPort` には載せていない。
    ///
    /// 成長(自己修復)の強さは現在のエネルギーで決まる。エネルギーはこのステップの
    /// 後に減衰するので、触れられずに放置し続けるほど、次第に修復が弱まっていく。
    /// ただし `MIN_GROWTH_SCALE` を下限とし、実測で見つかった崩壊の崖には触れさせない。
    pub fn step(&mut self) {
        let growth_scale = MIN_GROWTH_SCALE + (1.0 - MIN_GROWTH_SCALE) * self.energy;
        self.lenia.step(&mut self.field, growth_scale);
        self.energy = (self.energy - ENERGY_DECAY_PER_STEP).max(MIN_ENERGY);
    }

    /// 場の総量。体が生きているかの目安になる。
    pub fn mass(&self) -> f32 {
        self.field.mass()
    }

    /// 現在のエネルギー(0.0..=1.0)。放置されて弱っているかの目安になる。
    pub fn energy(&self) -> f32 {
        self.energy
    }

    /// 場を空にして生物を置き直す。エネルギーも満タンに戻す。
    ///
    /// Lenia はカオス系であり、摂動の強さをいくら絞っても、履歴次第で生物が
    /// 崩壊しうる。安全側に倒した強さでも起こりうるため、復帰の手段を体が持つ。
    pub fn revive(&mut self) {
        self.field.clear();
        self.field.place_centered(&self.animal.pattern);
        self.energy = MAX_ENERGY;
    }
}

impl BodyPort for LeniaBody {
    fn inject(&mut self, perturbation: Perturbation) {
        self.field.inject(&perturbation);
        self.energy = (self.energy + ENERGY_PER_TOUCH).min(MAX_ENERGY);
    }

    fn observe(&self) -> FieldView<'_> {
        self.field.view()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body::load_animal;
    use crate::body::perturbation::CellPos;

    fn orbium() -> LeniaBody {
        LeniaBody::new(load_animal("O2u").unwrap(), 32, 32)
    }

    #[test]
    fn a_new_body_carries_the_animal_pattern() {
        // Arrange / Act
        let body = orbium();

        // Assert
        assert!(body.mass() > 70.0, "the animal must be placed in the field");
    }

    #[test]
    fn a_new_body_starts_at_full_energy() {
        // Arrange / Act
        let body = orbium();

        // Assert
        assert_eq!(body.energy(), MAX_ENERGY);
    }

    #[test]
    fn inject_reaches_the_field_through_the_port() {
        // Arrange
        let mut body = orbium();
        let before = body.mass();

        // Act: 生物から離れた空きセルに注入する
        body.inject(Perturbation {
            at: CellPos { x: 2, y: 2 },
            radius: 3.0,
            amount: 0.5,
        });

        // Assert
        assert!(body.mass() > before);
    }

    #[test]
    fn inject_also_raises_energy() {
        // Arrange: まずエネルギーを使い切る
        let mut body = orbium();
        for _ in 0..10_000 {
            body.step();
        }
        assert_eq!(body.energy(), MIN_ENERGY, "energy must be able to bottom out");

        // Act
        body.inject(Perturbation {
            at: CellPos { x: 2, y: 2 },
            radius: 3.0,
            amount: 0.1,
        });

        // Assert
        assert!(body.energy() > MIN_ENERGY, "a touch must raise energy");
    }

    #[test]
    fn energy_never_exceeds_the_cap_no_matter_how_often_touched() {
        // Arrange
        let mut body = orbium();

        // Act: 何度も触れる
        for _ in 0..100 {
            body.inject(Perturbation {
                at: CellPos { x: 2, y: 2 },
                radius: 3.0,
                amount: 0.1,
            });
        }

        // Assert
        assert_eq!(body.energy(), MAX_ENERGY);
    }

    #[test]
    fn revive_restores_a_collapsed_body() {
        // Arrange: 場を焼き払って崩壊させる
        let mut body = orbium();
        for _ in 0..12 {
            for y in (0..32).step_by(4) {
                for x in (0..32).step_by(4) {
                    body.inject(Perturbation {
                        at: CellPos { x, y },
                        radius: 6.0,
                        amount: 1.0,
                    });
                }
            }
            body.step();
        }
        for _ in 0..600 {
            body.step();
        }
        let collapsed = body.mass();

        // Act
        body.revive();

        // Assert
        assert!(
            !(5.0..=120.0).contains(&collapsed),
            "the body must be off its normal state, got {collapsed}"
        );
        assert!((body.mass() - 76.86).abs() < 1.0, "revive must restore the original pattern");
    }

    #[test]
    fn revive_leaves_no_debris_behind() {
        // Arrange: 場の隅に残骸を作る
        let mut body = orbium();
        body.inject(Perturbation {
            at: CellPos { x: 0, y: 0 },
            radius: 3.0,
            amount: 1.0,
        });

        // Act
        body.revive();

        // Assert
        assert_eq!(body.observe().get(0, 0), 0.0, "the field must be cleared first");
    }

    #[test]
    fn a_long_neglected_body_survives_indefinitely_at_reduced_mass() {
        // Arrange: 一切触れずに長時間放置する(20000ステップ ≈ 22分)
        let mut body = orbium();
        let healthy_mass = body.mass();

        // Act
        for _ in 0..20_000 {
            body.step();
        }

        // Assert: 崩壊はしないが、健常時よりはっきり総量が下がる
        let neglected_mass = body.mass();
        assert!(
            neglected_mass > 40.0,
            "neglect must weaken, not kill, the body; got {neglected_mass}"
        );
        assert!(
            neglected_mass < healthy_mass * 0.98,
            "neglect must be measurably weaker than a healthy body; \
             healthy={healthy_mass} neglected={neglected_mass}"
        );
    }

    #[test]
    fn touching_a_neglected_body_restores_its_mass_toward_healthy() {
        // Arrange: 放置してエネルギーを使い切る
        let mut body = orbium();
        for _ in 0..20_000 {
            body.step();
        }
        let neglected_mass = body.mass();

        // Act: 生物から離れた位置に触れ続けてエネルギーを回復させる
        for _ in 0..30 {
            body.inject(Perturbation {
                at: CellPos { x: 2, y: 2 },
                radius: 1.0,
                amount: 0.01,
            });
            for _ in 0..50 {
                body.step();
            }
        }

        // Assert: 最後の一撫でから 50 ステップぶん減衰しているので、厳密に上限とは
        // 一致しないが、ほぼ満タンまで回復しているはず
        assert!(
            body.energy() > 0.9,
            "repeated touches must refill energy close to the cap, got {}",
            body.energy()
        );
        assert!(
            body.mass() > neglected_mass,
            "restored energy must let the body recover mass; \
             neglected={neglected_mass} restored={}",
            body.mass()
        );
    }

    #[test]
    fn revive_also_restores_full_energy() {
        // Arrange: エネルギーを使い切ってから焼き払う
        let mut body = orbium();
        for _ in 0..10_000 {
            body.step();
        }

        // Act
        body.revive();

        // Assert
        assert_eq!(body.energy(), MAX_ENERGY);
    }
}
