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
        0.5 * (1.0 + crate::math::cosf(core::f32::consts::PI * distance / self.radius))
    }
}

/// トーラス上のセル配列へ、山型の摂動を加算する。
///
/// `body::Field::inject` と、入力を可視化するためだけの `render::TouchEcho::touch` が
/// この処理を共有する。両者は加算先の配列とクランプ範囲が違うだけで、
/// 「山型の重みで加算し、端は反対側へ折り返す」という処理そのものは同じであるべき
/// (予測可能性: 同じ処理は同じパターンで書く)。
///
/// `toroidal` を false にすると端で折り返さない(範囲外は捨てる)。壁で跳ね返る
/// 箱の体(粒子の体)の echo への記録に使う。トーラス折り返しだと、壁際に置かれた
/// 摂動の外周が場の反対側へ漏れて見える
pub fn accumulate_into_toroidal(
    cells: &mut [f32],
    width: usize,
    height: usize,
    perturbation: &Perturbation,
    min: f32,
    max: f32,
    toroidal: bool,
) {
    let reach = crate::math::ceilf(perturbation.radius) as i32;
    if reach <= 0 {
        return;
    }
    let (signed_width, signed_height) = (width as i32, height as i32);

    for dy in -reach..=reach {
        for dx in -reach..=reach {
            let distance = crate::math::sqrtf((dx * dx + dy * dy) as f32);
            let weight = perturbation.weight_at(distance);
            if weight <= 0.0 {
                continue;
            }
            if toroidal {
                let x = (perturbation.at.x as i32 + dx).rem_euclid(signed_width) as usize;
                let y = (perturbation.at.y as i32 + dy).rem_euclid(signed_height) as usize;
                let index = y * width + x;
                cells[index] = (cells[index] + perturbation.amount * weight).clamp(min, max);
            } else {
                let x = perturbation.at.x as i32 + dx;
                let y = perturbation.at.y as i32 + dy;
                if x < 0 || y < 0 || x >= signed_width || y >= signed_height {
                    continue;
                }
                let index = y as usize * width + x as usize;
                cells[index] = (cells[index] + perturbation.amount * weight).clamp(min, max);
            }
        }
    }
}

/// トーラス折り返しで加算する(`accumulate_into_toroidal` の既定の使い方)。
pub fn accumulate_into(
    cells: &mut [f32],
    width: usize,
    height: usize,
    perturbation: &Perturbation,
    min: f32,
    max: f32,
) {
    accumulate_into_toroidal(cells, width, height, perturbation, min, max, true)
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
        let samples: Vec<f32> = (0..=8)
            .map(|i| perturbation.weight_at(i as f32 * 0.5))
            .collect();

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

    #[test]
    fn accumulate_into_raises_the_centre_most() {
        // Arrange
        let mut cells = vec![0.0f32; 16 * 16];

        // Act
        accumulate_into(
            &mut cells,
            16,
            16,
            &Perturbation {
                at: CellPos { x: 8, y: 8 },
                radius: 3.0,
                amount: 0.5,
            },
            0.0,
            1.0,
        );

        // Assert
        assert!((cells[8 * 16 + 8] - 0.5).abs() < 1e-5);
        assert!(cells[8 * 16 + 9] < cells[8 * 16 + 8]);
        assert_eq!(
            cells[8 * 16 + 12],
            0.0,
            "outside the radius must stay untouched"
        );
    }

    #[test]
    fn accumulate_into_wraps_around_the_torus() {
        // Arrange: 場の端に注入する
        let mut cells = vec![0.0f32; 16 * 16];

        // Act
        accumulate_into(
            &mut cells,
            16,
            16,
            &Perturbation {
                at: CellPos { x: 0, y: 0 },
                radius: 3.0,
                amount: 0.5,
            },
            0.0,
            1.0,
        );

        // Assert: 反対側の端にも回り込んでいる
        assert!(cells[15] > 0.0, "the blob must wrap to the far edge");
        assert!(cells[15 * 16] > 0.0);
    }

    #[test]
    fn accumulate_into_respects_the_given_clamp_range() {
        // Arrange: すでに上限いっぱいの配列へさらに加算する
        let mut cells = vec![1.0f32; 16 * 16];

        // Act
        accumulate_into(
            &mut cells,
            16,
            16,
            &Perturbation {
                at: CellPos { x: 8, y: 8 },
                radius: 3.0,
                amount: 5.0,
            },
            0.0,
            1.0,
        );

        // Assert
        assert_eq!(cells[8 * 16 + 8], 1.0);
    }
}
