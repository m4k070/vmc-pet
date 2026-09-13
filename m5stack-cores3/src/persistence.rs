//! 記憶(`vmc_pet_body::memory`)を内蔵フラッシュに繋ぐ、M5Stack 固有の層。
//!
//! PC版(src/persistence.rs)がファイルシステムと OS の時計に繋ぐのと同じ
//! 位置にある。「何を持ち越すか」は体の側が決め、ここが持つのは
//! 「どこへ、どう置くか」だけ。
//!
//! # 消去回数という制約
//!
//! フラッシュは4KBのセクタ単位でしか消去できず、消去回数の上限はおおむね
//! 10万回である。PC版と同じ30秒ごとの保存を、毎回セクタ全消去でやると
//! 1か月ほど(2880回/日 → 35日)で使い切ってしまう。ファイルへの上書きと
//! 同じ感覚で書けない、というのがこの層の一番の違いである。
//!
//! そこで固定長のレコードを順に並べて埋め、セクタが満杯になったときだけ
//! 消去する。消去済みの領域には消去なしで書き込める(フラッシュの書き込みは
//! 1→0 の一方向で、0xFF から任意の値にはできる)ため成立する。
//!
//! レコードは当初16バイト(エネルギーだけ)で256区画入ったが、学んだ重みを
//! 保存するようになって44バイトになり、4KBのセクタに93区画になった。
//! 消去回数は93分の1で、30秒ごとの保存で 100000 × 93 × 30秒 ≒ 約8.8年もつ。
//!
//! 走査と符号化の中身は `vmc_pet_body::memory` にあり、PC 上でテストして
//! ある。ここに残っているのはフラッシュを実際に触る部分だけ。
//!
//! # 置き場所
//!
//! `partitions.csv` で切った `pet` パーティション。espflash の既定の表にある
//! `nvs` を流用しない理由はそちらのコメントに書いた。番地を直接書かずに
//! パーティション表を引くのは、表が変わったときに黙って他の領域を壊さない
//! ため。
//!
//! # 失敗の扱い
//!
//! PC版と同じで、記憶が読めない・書けないことはペットが生きられない理由には
//! しない。初回起動にはそもそも何も書かれていないし、フラッシュが壊れている
//! せいでペットが起動しないのは本末転倒なので、失敗しても記録して先へ進む。

use alloc::vec;

use embedded_storage::nor_flash::{NorFlash, ReadNorFlash};
use esp_bootloader_esp_idf::partitions::{
    read_partition_table, PartitionTable, PARTITION_TABLE_MAX_LEN,
};
use esp_storage::FlashStorage;
use vmc_pet_body::{scan_records, PetMemory, SavedMemory, RECORD_LEN};

/// `partitions.csv` で記憶用に切ったパーティションのラベル。
///
/// 種別(data/undefined)ではなくラベルで探す。同じ種別の領域は他にもあり得る
/// のに対し、ラベルはこちらが付けた名前なので取り違えようがない。
const PARTITION_LABEL: &str = "pet";

/// 記憶の読み書き。
pub struct MemoryStore {
    flash: FlashStorage<'static>,
    /// パーティションの先頭番地。パーティション表から引いた値で、
    /// ソースには書かない。
    offset: u32,
    /// パーティションの大きさ(バイト)。消去はこの全体に対して行う。
    ///
    /// 区画数×レコード長ではなくパーティションの大きさで消去するのは、
    /// レコード長(44バイト)が消去単位の4KBを割り切らず、区画数×レコード長では
    /// セクタ境界に揃わずに消去が失敗するため。
    length: usize,
    /// 並べられる区画の数。
    slots: usize,
    /// 次に書ける区画。`slots` と等しくなったら消去してから 0 に戻す。
    next_slot: usize,
    /// 起動時に見つけた記憶。領域全体の走査は起動時の一度だけで済ませ、
    /// 結果をここに持つ。
    found: Option<SavedMemory>,
    /// 書き込み失敗のログを一度だけ出すためのフラグ(PC版と同じ扱い)。
    warned: bool,
}

/// この層で起きうる失敗。どれも「記憶が使えない」に帰着するが、
/// 起動時のログで原因が分かるように区別してある。
#[derive(Debug)]
pub enum StoreError {
    /// パーティション表が読めなかった。
    UnreadablePartitionTable,
    /// `pet` パーティションがパーティション表に無い
    /// (古い表のまま書き込んだ場合。`--partition-table partitions.csv` が
    /// 渡っていないと起きる)。
    PartitionMissing,
    /// `pet` パーティションが1レコードぶんも無い。
    PartitionTooSmall,
    /// フラッシュの読み書きに失敗した。
    Flash,
}

impl MemoryStore {
    /// パーティション表から記憶の置き場所を探し、すでに書かれている
    /// 区画の数を数える。
    pub fn new(flash_peripheral: esp_hal::peripherals::FLASH<'static>) -> Result<Self, StoreError> {
        let mut flash = FlashStorage::new(flash_peripheral);

        let mut table_buffer = [0u8; PARTITION_TABLE_MAX_LEN];
        let table: PartitionTable<'_> = read_partition_table(&mut flash, &mut table_buffer)
            .map_err(|_| StoreError::UnreadablePartitionTable)?;
        let partition = table
            .iter()
            .find(|entry| entry.label_as_str() == PARTITION_LABEL)
            .ok_or(StoreError::PartitionMissing)?;
        let offset = partition.offset();
        let length = partition.len() as usize;
        if length < RECORD_LEN {
            return Err(StoreError::PartitionTooSmall);
        }
        let slots = length / RECORD_LEN;

        // 領域全体を一度だけ読んで走査する。ここが唯一の全域読み出しで、
        // 以降は書き込みたい区画だけを触る。
        let mut area = vec![0u8; slots * RECORD_LEN];
        flash
            .read(offset, &mut area)
            .map_err(|_| StoreError::Flash)?;
        let (found, next_slot) = scan_records(&area);

        Ok(Self {
            flash,
            offset,
            length,
            slots,
            next_slot,
            found,
            warned: false,
        })
    }

    /// 起動時に見つけた記憶。初回起動(まだ何も書かれていない)や、
    /// レコードが壊れていた場合は `None`。
    ///
    /// これは `new` が走査した結果で、`save` しても変わらない。記憶の復元は
    /// 起動時に一度だけ行うもの(`Pet::restore`)なので、走査も一度で足りる。
    pub fn load(&self) -> Option<SavedMemory> {
        self.found
    }

    /// 記憶を書き出す。失敗しても動作は続け、警告は一度だけ出す。
    pub fn save(&mut self, memory: PetMemory, now_unix_seconds: u64) {
        if let Err(error) = self.write(memory, now_unix_seconds) {
            if !self.warned {
                esp_println::println!("vmc-pet-cores3: could not save memory: {error:?}");
                self.warned = true;
            }
        }
    }

    fn write(&mut self, memory: PetMemory, now_unix_seconds: u64) -> Result<(), StoreError> {
        if self.next_slot >= self.slots {
            // 満杯。ここでだけセクタを消去する(256回に1回)。
            //
            // 消去はセクタ境界でしかできないため、パーティションの先頭と大きさが
            // 4KBの倍数でなければここで失敗する。`partitions.csv` の `pet` は
            // ちょうど1セクタに揃えてある。失敗した場合は以降保存されなくなるが、
            // 警告を出して動作自体は続く。
            let end = self.offset + self.length as u32;
            self.flash
                .erase(self.offset, end)
                .map_err(|_| StoreError::Flash)?;
            self.next_slot = 0;
        }

        let record = SavedMemory::new(memory, now_unix_seconds).encode();
        let at = self.offset + (self.next_slot * RECORD_LEN) as u32;
        self.flash
            .write(at, &record)
            .map_err(|_| StoreError::Flash)?;
        self.next_slot += 1;
        Ok(())
    }

    /// これまでに埋まった区画の数。起動時のログに出すためにある。
    pub fn used_slots(&self) -> usize {
        self.next_slot
    }

    /// 並べられる区画の数。
    pub fn slots(&self) -> usize {
        self.slots
    }
}
