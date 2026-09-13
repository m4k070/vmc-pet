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

use crate::care_prediction::{CareMemory, FEATURES};

/// 再起動をまたいで持ち越す状態。
///
/// 今後フィールドを足すときは `#[serde(default)]` を付ける。古い保存ファイルが
/// そのまま読めるようにするため。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PetMemory {
    /// 気分状態(0.0..=1.0)。1.0 が最も元気。
    pub energy: f32,
    /// 世話がいつ来るかについて学んだこと。これを保存するようになる前の
    /// ファイルには無いので、無ければ何も学んでいない状態として読む。
    #[serde(default)]
    pub care: CareMemory,
}

impl PetMemory {
    /// エネルギーだけを指定し、学んだことは何も無い記憶を作る。
    pub fn with_energy(energy: f32) -> Self {
        Self {
            energy,
            care: CareMemory::default(),
        }
    }
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
///
/// ```text
/// [0..4]   目印 + 形式バージョン
/// [4..8]   保存時刻(Unix 秒、u32)
/// [8..12]  エネルギー(f32)
/// [12..40] 世話の予測の重み(f32 × 7)
/// [40..44] FNV-1a 検査値
/// ```
pub const RECORD_LEN: usize = 12 + 4 * FEATURES + 4;

/// レコードの先頭に置く目印と形式のバージョン。
///
/// 消去済みのフラッシュは全ビット1(0xFF)なので、それとは違う値でなければ
/// 「書かれていない場所」と区別できない。
const RECORD_MAGIC: u32 = 0x7065_7402; // "pet" + version 2

/// 形式バージョン1(エネルギーだけを持つ16バイトのレコード)の目印と長さ。
/// 学んだ重みを保存するようになる前に書かれたフラッシュから、エネルギーを
/// 引き継ぐためだけに読む(`scan_records` 参照)。
const LEGACY_RECORD_MAGIC: u32 = 0x7065_7401;
const LEGACY_RECORD_LEN: usize = 16;

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
        for (index, weight) in self.memory.care.weights.iter().enumerate() {
            let at = 12 + 4 * index;
            record[at..at + 4].copy_from_slice(&weight.to_le_bytes());
        }
        let body_end = RECORD_LEN - 4;
        let checksum = fnv1a(&record[0..body_end]);
        record[body_end..].copy_from_slice(&checksum.to_le_bytes());
        record
    }

    /// 固定長のバイト列から復号する。目印・検査値が合わないか、値が
    /// 範囲外なら `None`。
    pub fn decode(record: &[u8; RECORD_LEN]) -> Option<Self> {
        if read_u32(record, 0) != RECORD_MAGIC {
            return None;
        }
        let body_end = RECORD_LEN - 4;
        if read_u32(record, body_end) != fnv1a(&record[0..body_end]) {
            return None;
        }
        let saved_at_unix_seconds = read_u32(record, 4) as u64;
        let energy = read_f32(record, 8);
        let mut weights = [0.0f32; FEATURES];
        for (index, weight) in weights.iter_mut().enumerate() {
            *weight = read_f32(record, 12 + 4 * index);
        }
        // 検査値が通っていても、意味として有り得ない値は受け取らない。
        // NaN のエネルギーを体へ渡すと場ごと NaN に汚染され、NaN の重みは
        // 予測を通じて振る舞いを汚染する。
        if !(0.0..=1.0).contains(&energy) || !weights.iter().all(|weight| weight.is_finite()) {
            return None;
        }
        Some(Self {
            memory: PetMemory {
                energy,
                care: CareMemory { weights },
            },
            saved_at_unix_seconds,
        })
    }

    /// 形式バージョン1(エネルギーだけ)のレコードを復号する。学んだことは
    /// 何も無い記憶として読む。
    fn decode_legacy(record: &[u8; LEGACY_RECORD_LEN]) -> Option<Self> {
        if read_u32(record, 0) != LEGACY_RECORD_MAGIC
            || read_u32(record, 12) != fnv1a(&record[0..12])
        {
            return None;
        }
        let energy = read_f32(record, 8);
        if !(0.0..=1.0).contains(&energy) {
            return None;
        }
        Some(Self::new(
            PetMemory::with_energy(energy),
            read_u32(record, 4) as u64,
        ))
    }
}

fn read_u32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn read_f32(bytes: &[u8], at: usize) -> f32 {
    f32::from_bits(read_u32(bytes, at))
}

/// 消去済み(全ビット1)の区画か。
fn is_erased(record: &[u8]) -> bool {
    record.iter().all(|byte| *byte == 0xFF)
}

/// レコードを並べた領域を走査し、最後に書かれた記憶と、次に書ける場所を返す。
///
/// なぜ「1レコードを上書き」ではなく並べるのか: フラッシュは書き換えのたびに
/// 4KBのセクタ全体を消去する必要があり、消去回数には上限(おおむね10万回)が
/// ある。30秒ごとに保存すると1か月ほどで使い切ってしまう。一方、消去済みの
/// 領域へは消去なしで書き込める(1→0 の一方向なので)。そこでセクタを
/// `RECORD_LEN` バイトずつの区画に分けて順に埋め、満杯になったときだけ消去する。
/// 4KBのセクタなら93区画で、消去回数は93分の1になる。
///
/// フラッシュを触らない純粋な関数にしてあるのは、ここが一番間違えやすく、
/// 実機でしか動かない層に埋めるとテストできなくなるため。
///
/// 戻り値の2番目は次に書ける区画の番号で、区画数と等しいなら満杯(消去が要る)。
///
/// # 旧形式からの移行
///
/// 学んだ重みを保存するようになる前のフラッシュには、16バイトの旧形式の
/// レコードが並んでいる。新しい長さで区切って読むと境界がずれて全部壊れて
/// 見えるので、先頭が旧形式の目印なら旧形式として読み、最新のエネルギーを
/// 引き継ぐ。そのうえで「満杯」を返し、最初の保存でセクタを消去して新形式で
/// 書き直させる。
pub fn scan_records(area: &[u8]) -> (Option<SavedMemory>, usize) {
    let slots = area.len() / RECORD_LEN;
    if area.len() >= 4 && read_u32(area, 0) == LEGACY_RECORD_MAGIC {
        let latest = area
            .chunks_exact(LEGACY_RECORD_LEN)
            .take_while(|chunk| !is_erased(chunk))
            .filter_map(|chunk| {
                let record: &[u8; LEGACY_RECORD_LEN] = chunk.try_into().ok()?;
                SavedMemory::decode_legacy(record)
            })
            .last();
        return (latest, slots);
    }

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
        let saved = SavedMemory::new(PetMemory::with_energy(1.0), 1_000_000);

        // Act / Assert
        assert_eq!(saved.seconds_away(1_003_600), 3600.0);
    }

    #[test]
    fn a_clock_that_went_backwards_reports_no_time_away() {
        // Arrange: 保存時刻より前の「現在」を渡す(時計の巻き戻り)
        let saved = SavedMemory::new(PetMemory::with_energy(1.0), 1_000_000);

        // Act / Assert: 負にはせず 0 として扱う
        assert_eq!(saved.seconds_away(999_000), 0.0);
    }

    /// 学んだことを持つ記憶。重みはどれも既定値と違う値にしてある。
    fn memory_with_learning(energy: f32) -> PetMemory {
        let mut weights = [0.0f32; FEATURES];
        for (index, weight) in weights.iter_mut().enumerate() {
            *weight = -3.5 + index as f32 * 0.75;
        }
        PetMemory {
            energy,
            care: CareMemory { weights },
        }
    }

    /// 形式バージョン1(エネルギーだけ)のレコードを、当時と同じ並びで作る。
    fn legacy_record(energy: f32, saved_at: u32) -> [u8; LEGACY_RECORD_LEN] {
        let mut record = [0u8; LEGACY_RECORD_LEN];
        record[0..4].copy_from_slice(&LEGACY_RECORD_MAGIC.to_le_bytes());
        record[4..8].copy_from_slice(&saved_at.to_le_bytes());
        record[8..12].copy_from_slice(&energy.to_le_bytes());
        let checksum = fnv1a(&record[0..12]);
        record[12..16].copy_from_slice(&checksum.to_le_bytes());
        record
    }

    #[test]
    fn learned_weights_round_trip_through_bytes() {
        // Arrange
        let saved = SavedMemory::new(memory_with_learning(0.6), 1_700_000_000);

        // Act
        let decoded = SavedMemory::decode(&saved.encode()).expect("a fresh record must decode");

        // Assert: 学んだ重みも1ビットも欠けずに戻る
        assert_eq!(decoded, saved);
    }

    #[test]
    fn a_record_with_a_non_finite_weight_is_rejected() {
        // Arrange: 検査値まで正しい、しかし NaN の重みを持つレコード
        let mut memory = memory_with_learning(0.6);
        memory.care.weights[2] = f32::NAN;
        let record = SavedMemory::new(memory, 1_700_000_000).encode();

        // Act / Assert
        assert!(SavedMemory::decode(&record).is_none());
    }

    #[test]
    fn records_written_before_learning_existed_hand_over_their_energy() {
        // Arrange: 旧形式のレコードが3つ並んだフラッシュ
        let mut area = [0xFFu8; 4096];
        for (slot, energy) in [0.9f32, 0.6, 0.3].into_iter().enumerate() {
            let at = slot * LEGACY_RECORD_LEN;
            area[at..at + LEGACY_RECORD_LEN]
                .copy_from_slice(&legacy_record(energy, 1_700_000_000 + slot as u32));
        }

        // Act
        let (latest, next_slot) = scan_records(&area);

        // Assert: 最新のエネルギーを引き継ぎ、学んだことは何も無い。
        // 次の区画は「満杯」を指し、最初の保存でセクタを消去させる
        let latest = latest.expect("the legacy records must be read");
        assert_eq!(latest.memory.energy, 0.3);
        assert_eq!(latest.memory.care, CareMemory::default());
        assert_eq!(next_slot, 4096 / RECORD_LEN);
    }

    #[test]
    fn a_record_round_trips_through_bytes() {
        // Arrange
        let saved = SavedMemory::new(PetMemory::with_energy(0.42), 1_700_000_000);

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
        let mut record = SavedMemory::new(PetMemory::with_energy(0.5), 1_700_000_000).encode();
        record[9] ^= 0x01;

        // Act / Assert: 検査値が合わないので読まない
        assert!(SavedMemory::decode(&record).is_none());
    }

    #[test]
    fn an_impossible_energy_is_rejected_even_with_a_valid_checksum() {
        // Arrange: 検査値まで正しく作られた、しかし意味として有り得ないレコード。
        // NaN のエネルギーを体へ渡すと場ごと NaN に汚染されるため、
        // 形式の検査だけでは足りない
        let record = SavedMemory::new(PetMemory::with_energy(f32::NAN), 1_700_000_000).encode();

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
            let saved =
                SavedMemory::new(PetMemory::with_energy(energy), 1_700_000_000 + slot as u64);
            area[slot * RECORD_LEN..][..RECORD_LEN].copy_from_slice(&saved.encode());
        }

        // Act
        let (latest, next_slot) = scan_records(&area);

        // Assert: 一番新しいもの(最後に書いたもの)を返し、次は4番目の区画
        assert_eq!(
            latest.expect("three records were written").memory.energy,
            0.3
        );
        assert_eq!(next_slot, 3);
    }

    #[test]
    fn a_broken_last_record_falls_back_to_the_one_before_it() {
        // Arrange: 最後のレコードが書き込み中に電源断で壊れた状態
        let mut area = [0xFFu8; RECORD_LEN * 4];
        let good = SavedMemory::new(PetMemory::with_energy(0.7), 1_700_000_000);
        area[0..RECORD_LEN].copy_from_slice(&good.encode());
        let mut broken = SavedMemory::new(PetMemory::with_energy(0.2), 1_700_000_100).encode();
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
            let saved = SavedMemory::new(PetMemory::with_energy(0.5), 1_700_000_000);
            area[slot * RECORD_LEN..][..RECORD_LEN].copy_from_slice(&saved.encode());
        }

        // Act / Assert: 区画数と等しい = 消去が要る
        assert_eq!(scan_records(&area).1, 3);
    }
}
