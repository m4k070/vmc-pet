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

/// フラッシュ上の1レコードの大きさ(バイト)。
///
/// 4の倍数にしてあるのは、ESP32-S3 のフラッシュが4バイト単位でしか読み書き
/// できないため(`esp_storage::FlashStorage::WORD_SIZE`)。
pub const RECORD_LEN: usize = 16;

/// レコードの先頭に置く目印と形式のバージョン。
///
/// 消去済みのフラッシュは全ビット1(0xFF)なので、それとは違う値でなければ
/// 「書かれていない場所」と区別できない。
const RECORD_MAGIC: u32 = 0x7065_7401; // "pet" + version 1

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

    /// 固定長のバイト列へ符号化する。生のフラッシュへ書くための形式。
    ///
    /// PC版が JSON なのに対してこちらを固定長のバイナリにしたのは、置き場所の
    /// 性質が違うため:
    ///
    /// - ファイルシステムが無いので「どこまでがデータか」を教えてくれるものが
    ///   ない。固定長なら長さを知らなくても読める
    /// - 書き込み中の電源断で、途中まで書かれたレコードが残りうる。検査値を
    ///   持たせて、壊れたレコードを読まずに済むようにする
    /// - JSON(可変長・文字列)は人が覗けるのが利点だが、フラッシュ上の生の
    ///   バイト列にその利点は無い
    pub fn encode(&self) -> [u8; RECORD_LEN] {
        let mut record = [0u8; RECORD_LEN];
        record[0..4].copy_from_slice(&RECORD_MAGIC.to_le_bytes());
        // 秒を u32 に落とす。RTC(BM8563)のカレンダー自体が2099年までしか
        // 表現できないので、u32(2106年まで)で足りる。
        record[4..8].copy_from_slice(&(self.saved_at_unix_seconds as u32).to_le_bytes());
        record[8..12].copy_from_slice(&self.memory.energy.to_le_bytes());
        let checksum = fnv1a(&record[0..12]);
        record[12..16].copy_from_slice(&checksum.to_le_bytes());
        record
    }

    /// 固定長のバイト列から復号する。目印・検査値が合わないか、値が
    /// 範囲外なら `None`。
    pub fn decode(record: &[u8; RECORD_LEN]) -> Option<Self> {
        if u32::from_le_bytes(record[0..4].try_into().ok()?) != RECORD_MAGIC {
            return None;
        }
        if u32::from_le_bytes(record[12..16].try_into().ok()?) != fnv1a(&record[0..12]) {
            return None;
        }
        let saved_at_unix_seconds = u32::from_le_bytes(record[4..8].try_into().ok()?) as u64;
        let energy = f32::from_le_bytes(record[8..12].try_into().ok()?);
        // 検査値が通っていても、意味として有り得ない値は受け取らない。
        // NaN のエネルギーを体へ渡すと場ごと NaN に汚染される。
        if !(0.0..=1.0).contains(&energy) {
            return None;
        }
        Some(Self {
            memory: PetMemory { energy },
            saved_at_unix_seconds,
        })
    }
}

/// 消去済み(全ビット1)のレコードか。
fn is_erased(record: &[u8; RECORD_LEN]) -> bool {
    record.iter().all(|byte| *byte == 0xFF)
}

/// レコードを並べた領域を走査し、最後に書かれた記憶と、次に書ける場所を返す。
///
/// なぜ「1レコードを上書き」ではなく並べるのか: フラッシュは書き換えのたびに
/// 4KBのセクタ全体を消去する必要があり、消去回数には上限(おおむね10万回)が
/// ある。30秒ごとに保存すると1か月ほどで使い切ってしまう。一方、消去済みの
/// 領域へは消去なしで書き込める(1→0 の一方向なので)。そこでセクタを16バイト
/// ずつの区画に分けて順に埋め、満杯になったときだけ消去する。区画が256個
/// あれば、消去回数は256分の1になる。
///
/// フラッシュを触らない純粋な関数にしてあるのは、ここが一番間違えやすく、
/// 実機でしか動かない層に埋めるとテストできなくなるため。
///
/// 戻り値の2番目は次に書ける区画の番号で、区画数と等しいなら満杯(消去が要る)。
pub fn scan_records(area: &[u8]) -> (Option<SavedMemory>, usize) {
    let mut latest = None;
    let mut next_slot = 0;
    for (slot, chunk) in area.chunks_exact(RECORD_LEN).enumerate() {
        let record: &[u8; RECORD_LEN] = chunk.try_into().expect("chunks_exact yields RECORD_LEN");
        if is_erased(record) {
            break;
        }
        next_slot = slot + 1;
        // 壊れたレコード(電源断で途中まで書かれたもの)は読み飛ばすが、
        // 書かれてはいるので次の区画へは進める。
        if let Some(decoded) = SavedMemory::decode(record) {
            latest = Some(decoded);
        }
    }
    (latest, next_slot)
}

/// FNV-1a 32bit ハッシュ。テーブルを持たない数行で書けるので、
/// 外部クレートを足さずに検査値として使う(暗号用途ではない)。
fn fnv1a(bytes: &[u8]) -> u32 {
    let mut hash: u32 = 0x811C_9DC5;
    for byte in bytes {
        hash ^= *byte as u32;
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
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

    #[test]
    fn a_record_round_trips_through_bytes() {
        // Arrange
        let saved = SavedMemory::new(PetMemory { energy: 0.42 }, 1_700_000_000);

        // Act
        let decoded = SavedMemory::decode(&saved.encode()).expect("a fresh record must decode");

        // Assert
        assert_eq!(decoded, saved);
    }

    #[test]
    fn an_erased_record_is_not_mistaken_for_data() {
        // Arrange: 消去済みのフラッシュは全ビット1
        let erased = [0xFFu8; RECORD_LEN];

        // Act / Assert
        assert!(SavedMemory::decode(&erased).is_none());
    }

    #[test]
    fn a_corrupted_record_is_rejected() {
        // Arrange: 1バイトだけ壊す(書き込み中の電源断に相当)
        let mut record = SavedMemory::new(PetMemory { energy: 0.5 }, 1_700_000_000).encode();
        record[9] ^= 0x01;

        // Act / Assert: 検査値が合わないので読まない
        assert!(SavedMemory::decode(&record).is_none());
    }

    #[test]
    fn an_impossible_energy_is_rejected_even_with_a_valid_checksum() {
        // Arrange: 検査値まで正しく作られた、しかし意味として有り得ないレコード。
        // NaN のエネルギーを体へ渡すと場ごと NaN に汚染されるため、
        // 形式の検査だけでは足りない
        let record = SavedMemory::new(PetMemory { energy: f32::NAN }, 1_700_000_000).encode();

        // Act / Assert
        assert!(SavedMemory::decode(&record).is_none());
    }

    #[test]
    fn scanning_an_untouched_area_finds_nothing_and_starts_at_the_first_slot() {
        // Arrange: 初回起動(セクタ全体が消去済み)
        let area = [0xFFu8; RECORD_LEN * 4];

        // Act / Assert
        assert_eq!(scan_records(&area), (None, 0));
    }

    #[test]
    fn scanning_returns_the_last_written_record() {
        // Arrange: 3つ書かれた状態
        let mut area = [0xFFu8; RECORD_LEN * 4];
        for (slot, energy) in [0.9, 0.6, 0.3].into_iter().enumerate() {
            let saved = SavedMemory::new(PetMemory { energy }, 1_700_000_000 + slot as u64);
            area[slot * RECORD_LEN..][..RECORD_LEN].copy_from_slice(&saved.encode());
        }

        // Act
        let (latest, next_slot) = scan_records(&area);

        // Assert: 一番新しいもの(最後に書いたもの)を返し、次は4番目の区画
        assert_eq!(latest.expect("three records were written").memory.energy, 0.3);
        assert_eq!(next_slot, 3);
    }

    #[test]
    fn a_broken_last_record_falls_back_to_the_one_before_it() {
        // Arrange: 最後のレコードが書き込み中に電源断で壊れた状態
        let mut area = [0xFFu8; RECORD_LEN * 4];
        let good = SavedMemory::new(PetMemory { energy: 0.7 }, 1_700_000_000);
        area[0..RECORD_LEN].copy_from_slice(&good.encode());
        let mut broken = SavedMemory::new(PetMemory { energy: 0.2 }, 1_700_000_100).encode();
        broken[4] ^= 0xAA;
        area[RECORD_LEN..][..RECORD_LEN].copy_from_slice(&broken);

        // Act
        let (latest, next_slot) = scan_records(&area);

        // Assert: 壊れた1つ前へ戻るが、区画は消費済みとして次へ進む
        assert_eq!(latest, Some(good));
        assert_eq!(next_slot, 2);
    }

    #[test]
    fn a_full_area_reports_a_slot_past_the_end_so_the_caller_knows_to_erase() {
        // Arrange: 全区画が埋まっている
        let mut area = [0xFFu8; RECORD_LEN * 3];
        for slot in 0..3 {
            let saved = SavedMemory::new(PetMemory { energy: 0.5 }, 1_700_000_000);
            area[slot * RECORD_LEN..][..RECORD_LEN].copy_from_slice(&saved.encode());
        }

        // Act / Assert: 区画数と等しい = 消去が要る
        assert_eq!(scan_records(&area).1, 3);
    }
}
