//! 同じ場所への刺激に慣れる(応答が弱まる)仕組み。
//!
//! docs/DESIGN.md「経験に基づいて変化する生き物」のロードマップのうち、
//! 「慣れ」に相当する。触れられた場所ごとに慣れの度合いを覚えておき、
//! その場所への次の刺激を弱める。触れるのをやめれば時間とともに回復する。
//!
//! **場所ごとに持つ**のは、生物の慣れが刺激ごとに固有(ある音に慣れても
//! 別の音には反応する)であることに倣っている。同じところを叩き続けると
//! 効かなくなるが、別のところを叩けばちゃんと反応する、という形になる。
//!
//! `TouchEcho`(触れた跡を覚えて減衰させる、見た目専用のデータ)と構造は
//! よく似ているが、あえて別に持っている:
//!
//! - echo の時定数は意図的に速く、離すと1〜2秒で消える
//!   (`touch_echo` のテスト参照)。慣れとしては速すぎる
//! - echo はホバーでも溜まる。ホバーは設計上、体に一切触れないため、
//!   体に届かない刺激で慣れが進むのは筋が通らない
//! - echo の減衰は描画フレームごとに進む。フレームレートは PC(約60fps)と
//!   M5Stack(約25回/s)で違うため、体に効く仕組みをこれに乗せると両機で
//!   挙動が変わってしまう。慣れは**体のステップ**(どちらも15/s)に乗せる
//!
//! いまのところ保存(プロセスをまたぐ記憶)には含めていない。回復が数十秒
//! なので、次に起動するまでにはどうせ回復しきっているため意味が薄い。
//! 「毎朝この辺りを触られている」ような長期の馴染みを作りたくなったら、
//! それはもっと遅い別の仕組みとして足すことになる。

use alloc::vec;
use alloc::vec::Vec;

use crate::{accumulate_into, CellPos, Perturbation};

/// 慣れの度合いの値域。0.0 は真新しい(刺激がそのまま効く)、
/// 1.0 は完全に慣れた(その場所への刺激がまったく効かない)。
const MIN_LEVEL: f32 = 0.0;
const MAX_LEVEL: f32 = 1.0;

/// 1回触れられるごとに、触れられた場所の慣れがどれだけ進むか。
/// 同じところを5回叩けば完全に慣れる(効かなくなる)勘定。
const LEVEL_PER_TOUCH: f32 = 0.2;

/// 体が1ステップ進むごとの回復(減衰)率。
///
/// 15 step/s なので、完全に慣れた状態(1.0)から 0.995^675 ≈ 0.03 まで、
/// つまり約45秒でほぼ元に戻る。1秒ごとに叩き続ければ飽和して効かなくなるが、
/// 5秒に1回くらいの間隔なら 0.6 程度で落ち着き、3〜4割は効き続ける。
const RECOVERY_PER_STEP: f32 = 0.995;

/// 場所ごとの慣れの度合い。
pub struct Habituation {
    width: usize,
    height: usize,
    level: Vec<f32>,
}

impl Habituation {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            level: vec![MIN_LEVEL; width * height],
        }
    }

    /// その場所の慣れの度合い(0.0..=1.0)。範囲外は 0.0。
    pub fn level_at(&self, at: CellPos) -> f32 {
        if at.x >= self.width || at.y >= self.height {
            return MIN_LEVEL;
        }
        self.level[at.y * self.width + at.x]
    }

    /// その場所の刺激がいまどれだけ効くか(1.0 = そのまま効く、0.0 = 効かない)。
    pub fn attention_at(&self, at: CellPos) -> f32 {
        MAX_LEVEL - self.level_at(at)
    }

    /// 触れられたことを覚える。
    ///
    /// 慣れが広がる範囲は、実際に体へ効いた摂動と同じ `at`/`radius` を使う
    /// (定数を二重に持つと、クリックの効き方を変えたときにずれるため)。
    /// 強さだけはこちらの `LEVEL_PER_TOUCH` を使う。
    pub fn record(&mut self, perturbation: &Perturbation) {
        accumulate_into(
            &mut self.level,
            self.width,
            self.height,
            &Perturbation {
                at: perturbation.at,
                radius: perturbation.radius,
                amount: LEVEL_PER_TOUCH,
            },
            MIN_LEVEL,
            MAX_LEVEL,
        );
    }

    /// 体が1ステップ進むごとに呼ぶ。触れられていない場所は慣れが薄れていく。
    pub fn recover(&mut self) {
        for level in &mut self.level {
            *level *= RECOVERY_PER_STEP;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn click_at(x: usize, y: usize) -> Perturbation {
        Perturbation {
            at: CellPos { x, y },
            radius: 4.5,
            amount: 0.20,
        }
    }

    #[test]
    fn a_fresh_place_lets_the_stimulus_through() {
        // Arrange / Act
        let habituation = Habituation::new(32, 32);

        // Assert
        assert_eq!(habituation.attention_at(CellPos { x: 8, y: 8 }), 1.0);
    }

    #[test]
    fn touching_the_same_place_makes_it_stop_registering() {
        // Arrange
        let mut habituation = Habituation::new(32, 32);
        let at = CellPos { x: 8, y: 8 };

        // Act: 同じ場所を5回叩く(間に回復を挟まない)
        for _ in 0..5 {
            habituation.record(&click_at(at.x, at.y));
        }

        // Assert: ほとんど効かなくなる
        assert!(
            habituation.attention_at(at) < 0.05,
            "got {}",
            habituation.attention_at(at)
        );
    }

    #[test]
    fn a_different_place_still_registers_fully() {
        // Arrange
        let mut habituation = Habituation::new(32, 32);

        // Act: 片方だけを叩き続ける
        for _ in 0..5 {
            habituation.record(&click_at(8, 8));
        }

        // Assert: 離れた場所は真新しいまま(刺激ごとに固有な慣れ)
        assert_eq!(habituation.attention_at(CellPos { x: 24, y: 24 }), 1.0);
    }

    #[test]
    fn habituation_fades_back_within_about_a_minute() {
        // Arrange: 完全に慣れさせる
        let mut habituation = Habituation::new(32, 32);
        let at = CellPos { x: 8, y: 8 };
        for _ in 0..5 {
            habituation.record(&click_at(at.x, at.y));
        }
        assert!(habituation.attention_at(at) < 0.05);

        // Act: 触れずに45秒ぶん(15 step/s で 675 ステップ)進める
        for _ in 0..675 {
            habituation.recover();
        }

        // Assert: ほぼ元どおり反応するようになる
        assert!(
            habituation.attention_at(at) > 0.9,
            "got {}",
            habituation.attention_at(at)
        );
    }

    #[test]
    fn the_level_never_leaves_its_range() {
        // Arrange
        let mut habituation = Habituation::new(32, 32);
        let at = CellPos { x: 8, y: 8 };

        // Act: 何度叩いても上限を超えない
        for _ in 0..100 {
            habituation.record(&click_at(at.x, at.y));
        }

        // Assert
        assert_eq!(habituation.level_at(at), MAX_LEVEL);
        assert_eq!(habituation.attention_at(at), MIN_LEVEL);
    }
}
