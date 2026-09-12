//! IF層が体に注入できる唯一の操作。
//!
//! 外界からの働きかけは、すべてこの型に還元してから場へ渡す。
//! 内部の CA ロジックを直接いじる経路を作らないための境界そのもの。

/// 場のセル座標。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellPos {
    pub x: usize,
    pub y: usize,
}

/// 場の一点を中心に、なだらかな山型でエネルギーを加減する操作。
///
/// 角の立った注入は Lenia の連続性を壊すため、中心から縁へ滑らかに 0 へ落とす。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Perturbation {
    pub at: CellPos,
    /// 影響が及ぶ半径(セル)。
    pub radius: f32,
    /// 中心に加える量。負なら削る。
    pub amount: f32,
}

impl Perturbation {
    /// 中心からの距離に対する重み。半径の外では 0。
    /// 余弦の山にすることで、縁で値と傾きの両方が滑らかに 0 へ落ちる。
    pub fn weight_at(&self, distance: f32) -> f32 {
        if self.radius <= 0.0 || distance >= self.radius {
            return 0.0;
        }
        0.5 * (1.0 + (std::f32::consts::PI * distance / self.radius).cos())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Perturbation {
        Perturbation {
            at: CellPos { x: 5, y: 5 },
            radius: 4.0,
            amount: 0.5,
        }
    }

    #[test]
    fn weight_is_one_at_the_centre_and_zero_at_the_rim() {
        // Arrange
        let perturbation = sample();

        // Act / Assert
        assert!((perturbation.weight_at(0.0) - 1.0).abs() < f32::EPSILON);
        assert_eq!(perturbation.weight_at(4.0), 0.0);
        assert_eq!(perturbation.weight_at(9.0), 0.0);
    }

    #[test]
    fn weight_falls_off_monotonically() {
        // Arrange
        let perturbation = sample();

        // Act
        let samples: Vec<f32> = (0..=8).map(|i| perturbation.weight_at(i as f32 * 0.5)).collect();

        // Assert
        for pair in samples.windows(2) {
            assert!(pair[1] <= pair[0], "weight must not increase: {pair:?}");
        }
    }

    #[test]
    fn a_zero_radius_perturbation_does_nothing() {
        // Arrange
        let perturbation = Perturbation {
            at: CellPos { x: 0, y: 0 },
            radius: 0.0,
            amount: 1.0,
        };

        // Act / Assert
        assert_eq!(perturbation.weight_at(0.0), 0.0);
    }
}
