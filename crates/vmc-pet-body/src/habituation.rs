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
//! # 「場所」は体を基準にする
//!
//! 慣れは場(世界)の座標ではなく、**体の中心(重心)からの相対位置**で覚える。
//! 同じ場所とは「体の同じ部位」のことで、世界の同じ座標のことではない。
//!
//! 最初は場の座標で覚えていたが、M5Stack で同じ位置を触り続けても慣れが
//! ほとんど溜まらなかった。計測すると、既定の生物(O2u)は1秒に約9セル
//! 滑るように移動しており、カメラがそれを追うので、画面上の同じ位置を
//! 1秒おきに触っても場の座標では毎回9セル先を触っていた(7回目でも
//! 92〜96%効く)。PC ではマウスで素早く連打すれば移動が小さいうちに
//! 叩き終わるので気づかなかった。
//!
//! カメラは重心を画面中央に保つので、体基準の座標はほぼ画面上の位置と
//! 一致する。ユーザーが「同じところ」と感じる位置と、ここが同じ場所と
//! みなす位置が揃う。
//!
//! # 慣れは触れた範囲より広く記録する
//!
//! 指のタッチは同じ位置を狙っても数セル(M5Stack で1セル≈1mm)ずれる。
//! 触れた範囲そのもの(半径4.5セル)に記録すると、±2セルのブレで7回目でも
//! 60%効いてしまう。記録を1.5倍(6.75セル)に広げると38%まで下がり、
//! それでも7セル離れれば新しい場所として満額で効く。2倍にすると体の広い
//! 範囲がまとめて慣れてしまう(9セル以上離れないと満額にならない)ため、
//! 1.5倍にした。
//!
//! # `TouchEcho` とは別に持つ理由
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

/// 触れた範囲(摂動の半径)に対して、慣れを何倍の半径まで記録するか。
/// タッチのブレを吸収するため(モジュール先頭のコメント参照)。
const SPREAD_BEYOND_TOUCH: f32 = 1.5;

/// 体が1ステップ進むごとの回復(減衰)率。
///
/// 15 step/s なので、完全に慣れた状態(1.0)から 0.995^675 ≈ 0.03 まで、
/// つまり約45秒でほぼ元に戻る。1秒ごとに叩き続ければ飽和して効かなくなるが、
/// 5秒に1回くらいの間隔なら 0.6 程度で落ち着き、3〜4割は効き続ける。
const RECOVERY_PER_STEP: f32 = 0.995;

/// 体の部位ごとの慣れの度合い。
///
/// 内部の配列は体の中心を原点とした座標で並んでいる。外からは場の座標と
/// そのときの体の中心を渡してもらい、ここで体基準へ変換する。
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
    ///
    /// `at` は場の座標、`body_centre` はそのときの体の中心(場の座標)。
    pub fn level_at(&self, at: CellPos, body_centre: CellPos) -> f32 {
        if at.x >= self.width || at.y >= self.height {
            return MIN_LEVEL;
        }
        let on_body = self.on_body(at, body_centre);
        self.level[on_body.y * self.width + on_body.x]
    }

    /// その場所の刺激がいまどれだけ効くか(1.0 = そのまま効く、0.0 = 効かない)。
    pub fn attention_at(&self, at: CellPos, body_centre: CellPos) -> f32 {
        MAX_LEVEL - self.level_at(at, body_centre)
    }

    /// 触れられたことを覚える。
    ///
    /// 慣れの中心と基準の広さは、実際に体へ効いた摂動と同じ `at`/`radius` を
    /// 使う(定数を二重に持つと、クリックの効き方を変えたときにずれるため)。
    /// そこから `SPREAD_BEYOND_TOUCH` 倍に広げて記録する。強さはこちらの
    /// `LEVEL_PER_TOUCH` を使う。
    pub fn record(&mut self, perturbation: &Perturbation, body_centre: CellPos) {
        let on_body = self.on_body(perturbation.at, body_centre);
        accumulate_into(
            &mut self.level,
            self.width,
            self.height,
            &Perturbation {
                at: on_body,
                radius: perturbation.radius * SPREAD_BEYOND_TOUCH,
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

    /// 場の座標を、体の中心からの相対位置(トーラス上で折り返した非負の座標)へ
    /// 変換する。
    fn on_body(&self, at: CellPos, body_centre: CellPos) -> CellPos {
        CellPos {
            x: (at.x + self.width - body_centre.x % self.width) % self.width,
            y: (at.y + self.height - body_centre.y % self.height) % self.height,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 体が動かない場合の体の中心。場の座標と体基準の座標が一致する。
    const STILL: CellPos = CellPos { x: 0, y: 0 };

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
        assert_eq!(habituation.attention_at(CellPos { x: 8, y: 8 }, STILL), 1.0);
    }

    #[test]
    fn touching_the_same_place_makes_it_stop_registering() {
        // Arrange
        let mut habituation = Habituation::new(32, 32);
        let at = CellPos { x: 8, y: 8 };

        // Act: 同じ場所を5回叩く(間に回復を挟まない)
        for _ in 0..5 {
            habituation.record(&click_at(at.x, at.y), STILL);
        }

        // Assert: ほとんど効かなくなる
        let attention = habituation.attention_at(at, STILL);
        assert!(attention < 0.05, "got {attention}");
    }

    #[test]
    fn a_different_place_still_registers_fully() {
        // Arrange
        let mut habituation = Habituation::new(32, 32);

        // Act: 片方だけを叩き続ける
        for _ in 0..5 {
            habituation.record(&click_at(8, 8), STILL);
        }

        // Assert: 離れた場所は真新しいまま(刺激ごとに固有な慣れ)
        assert_eq!(
            habituation.attention_at(CellPos { x: 24, y: 24 }, STILL),
            1.0
        );
    }

    #[test]
    fn a_touch_that_lands_a_few_cells_off_still_counts_as_the_same_place() {
        // Arrange: 同じ場所を5回叩いて慣れさせる
        let mut habituation = Habituation::new(32, 32);
        for _ in 0..5 {
            habituation.record(&click_at(8, 8), STILL);
        }

        // Act: 指のブレで3セルずれた位置を触る
        let attention = habituation.attention_at(CellPos { x: 11, y: 8 }, STILL);

        // Assert: 同じ場所としてかなり慣れている。触れた範囲そのもの
        // (4.5セル)にだけ記録すると、ここは 0.75 効いてしまっていた
        assert!(attention < 0.5, "got {attention}");
    }

    #[test]
    fn the_same_spot_on_a_moving_body_is_still_the_same_place() {
        // Arrange: 体の中心から見て同じ部位(右に5セル)を、体が1回ごとに
        // 9セル進む間に叩き続ける(O2u が1秒に進む距離)
        let mut habituation = Habituation::new(43, 32);
        for tap in 0..5 {
            let centre = CellPos {
                x: (tap * 9) % 43,
                y: 16,
            };
            let at = CellPos {
                x: (centre.x + 5) % 43,
                y: 16,
            };
            habituation.record(
                &Perturbation {
                    at,
                    radius: 4.5,
                    amount: 0.20,
                },
                centre,
            );
        }

        // Act: さらに9セル進んだ体の、同じ部位
        let centre = CellPos { x: 45 % 43, y: 16 };
        let attention = habituation.attention_at(
            CellPos {
                x: (centre.x + 5) % 43,
                y: 16,
            },
            centre,
        );

        // Assert: 世界の座標では毎回違う場所だが、体の同じ部位なので慣れている
        assert!(attention < 0.05, "got {attention}");
    }

    #[test]
    fn the_same_spot_in_the_world_is_a_new_place_once_the_body_has_moved_away() {
        // Arrange: 動かない体のある部位に慣れさせる
        let mut habituation = Habituation::new(32, 32);
        let at = CellPos { x: 8, y: 8 };
        for _ in 0..5 {
            habituation.record(&click_at(at.x, at.y), STILL);
        }

        // Act: 体が大きく移動したあと、世界の同じ座標を触る
        let moved = CellPos { x: 16, y: 16 };

        // Assert: そこはもう体の別の部位なので、満額で効く
        assert_eq!(habituation.attention_at(at, moved), 1.0);
    }

    #[test]
    fn habituation_fades_back_within_about_a_minute() {
        // Arrange: 完全に慣れさせる
        let mut habituation = Habituation::new(32, 32);
        let at = CellPos { x: 8, y: 8 };
        for _ in 0..5 {
            habituation.record(&click_at(at.x, at.y), STILL);
        }
        assert!(habituation.attention_at(at, STILL) < 0.05);

        // Act: 触れずに45秒ぶん(15 step/s で 675 ステップ)進める
        for _ in 0..675 {
            habituation.recover();
        }

        // Assert: ほぼ元どおり反応するようになる
        let attention = habituation.attention_at(at, STILL);
        assert!(attention > 0.9, "got {attention}");
    }

    #[test]
    fn the_level_never_leaves_its_range() {
        // Arrange
        let mut habituation = Habituation::new(32, 32);
        let at = CellPos { x: 8, y: 8 };

        // Act: 何度叩いても上限を超えない
        for _ in 0..100 {
            habituation.record(&click_at(at.x, at.y), STILL);
        }

        // Assert
        assert_eq!(habituation.level_at(at, STILL), MAX_LEVEL);
        assert_eq!(habituation.attention_at(at, STILL), MIN_LEVEL);
    }
}
