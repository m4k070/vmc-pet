//! Lenia を体として実装したもの。
//!
//! `BodyPort` の実装をここに閉じることで、体を差し替える実験の足場になる。
//! 別の CA や別の力学で体を作るなら、同じ窓口を実装した型を用意すればよい。

use super::{Animal, BodyPort, Field, FieldView, Lenia, Perturbation};

/// Lenia の場を体として持つ個体。
pub struct LeniaBody {
    /// 崩壊したときに置き直すため、元の生物を持ち続ける。
    animal: Animal,
    field: Field,
    lenia: Lenia,
}

impl LeniaBody {
    /// 生物を場の中央に配置して体を作る。
    pub fn new(animal: Animal, width: usize, height: usize) -> Self {
        let lenia = Lenia::new(animal.params.clone());
        let mut body = Self {
            animal,
            field: Field::new(width, height),
            lenia,
        };
        body.revive();
        body
    }

    /// 体の時間を1ステップ進める。
    /// 外界から呼べる操作ではないため、`BodyPort` には載せていない。
    pub fn step(&mut self) {
        self.lenia.step(&mut self.field);
    }

    /// 場の総量。体が生きているかの目安になる。
    pub fn mass(&self) -> f32 {
        self.field.mass()
    }

    /// 場を空にして生物を置き直す。
    ///
    /// Lenia はカオス系であり、摂動の強さをいくら絞っても、履歴次第で生物が
    /// 崩壊しうる。安全側に倒した強さでも起こりうるため、復帰の手段を体が持つ。
    pub fn revive(&mut self) {
        self.field.clear();
        self.field.place_centered(&self.animal.pattern);
    }
}

impl BodyPort for LeniaBody {
    fn inject(&mut self, perturbation: Perturbation) {
        self.field.inject(&perturbation);
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
}
