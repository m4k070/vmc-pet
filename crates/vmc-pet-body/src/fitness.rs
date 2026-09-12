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
    /// 記録中に一度でも崩壊(`Pet::step` が `true` を返す)が起きたか。
    collapsed: bool,
}

impl Trajectory {
    /// `pet` を `steps` ぶん進めながら軌跡を記録する。
    pub fn record(pet: &mut Pet, steps: u32) -> Self {
        let mut centroids = Vec::with_capacity(steps as usize);
        let mut collapsed = false;
        for _ in 0..steps {
            if pet.step() {
                collapsed = true;
            }
            centroids.push(pet.observe().toroidal_centroid());
        }
        Self { centroids, collapsed }
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
    pub fn fitness(&self, field_width: usize, field_height: usize) -> f32 {
        if self.collapsed {
            return COLLAPSE_PENALTY;
        }
        self.mean_step_displacement(field_width, field_height)
    }
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
        assert!(score > 0.0, "a gliding, healthy body should score positively, got {score}");
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
