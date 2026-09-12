//! プロセスをまたいで持ち越す記憶。
//!
//! これまでのペットは、再起動すると何も覚えていなかった(エネルギーは満タンに
//! 戻り、体も元の生物に置き直され、echo も消える)。どんな経験をしても、
//! プロセスが終われば全部忘れる状態だった。「経験に基づいて変化する生き物」
//! という目標に対して一番根本的に欠けていたのは、学習の仕組みではなく、
//! プロセスより長生きする記憶の器そのものだった(docs/DESIGN.md
//! 「プロセスをまたぐ記憶」参照)。
//!
//! 保存するのは「ゆっくり動く状態」だけで、場(Lenia の盤面)は含めない。
//! Lenia はカオス系で、置き直しても数秒で同じ姿の個体へ収束する(アトラクタ)
//! ため、毎回数KBを書き込むコストに対して得られる意味が薄いと判断した。
//! 「昨日かまってあげた分だけ今日の様子が違う」という手触りを作るのは、
//! 盤面の細部ではなくこのゆっくり動く状態の方である。
//!
//! ここは「いつ保存したか」を数値(Unix 時刻)として持つだけで、時計そのものは
//! 読まない。実際に時計を読むのはプラットフォーム側(PC は `std::time`、
//! M5Stack は RTC)で、この境界は `Pet` が時計を知らないという既存の方針
//! (pet.rs 参照)と揃えてある。

use serde::{Deserialize, Serialize};

/// 再起動をまたいで持ち越す状態。
///
/// 今後フィールドを足すときは `#[serde(default)]` を付ける。古い保存ファイルが
/// そのまま読めるようにするため(次に足す予定のものは「慣れ」の状態)。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PetMemory {
    /// 気分状態(0.0..=1.0)。1.0 が最も元気。
    pub energy: f32,
}

/// 保存ファイル(あるいはフラッシュ上のレコード)そのものの形。
/// `PetMemory` に「いつ保存したか」を添えたもの。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SavedMemory {
    pub memory: PetMemory,
    /// Unix 時刻(秒)。停止していた時間を求めるために要る。
    pub saved_at_unix_seconds: u64,
}

impl SavedMemory {
    pub fn new(memory: PetMemory, saved_at_unix_seconds: u64) -> Self {
        Self {
            memory,
            saved_at_unix_seconds,
        }
    }

    /// 保存してから `now_unix_seconds` までに経過した秒数。
    ///
    /// 時計が巻き戻っている場合(NTP による補正、RTC の電池切れなど)は 0 を
    /// 返す。システムの境界として、負の経過時間でエネルギーが増えるような
    /// 振る舞いは作らない。
    pub fn seconds_away(&self, now_unix_seconds: u64) -> f32 {
        now_unix_seconds.saturating_sub(self.saved_at_unix_seconds) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seconds_away_measures_the_gap_since_saving() {
        // Arrange: 1時間前に保存した記録
        let saved = SavedMemory::new(PetMemory { energy: 1.0 }, 1_000_000);

        // Act / Assert
        assert_eq!(saved.seconds_away(1_003_600), 3600.0);
    }

    #[test]
    fn a_clock_that_went_backwards_reports_no_time_away() {
        // Arrange: 保存時刻より前の「現在」を渡す(時計の巻き戻り)
        let saved = SavedMemory::new(PetMemory { energy: 1.0 }, 1_000_000);

        // Act / Assert: 負にはせず 0 として扱う
        assert_eq!(saved.seconds_away(999_000), 0.0);
    }
}
