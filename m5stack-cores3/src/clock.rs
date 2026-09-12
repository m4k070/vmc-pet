//! 電源が切れている間も進む時計。CoreS3 の BM8563(バッテリバックアップ付き
//! RTC、内部I²Cバス上)を読む。
//!
//! PC版(src/persistence.rs の `now_unix_seconds`)は OS の時計を読むだけで
//! 済むが、M5Stack には OS も NTP も無い。一方で記憶(`SavedMemory`)は
//! 「停止していた秒数」を要求する。`esp_hal` の内蔵 RTC カウンタは電源が
//! 切れると 0 に戻るため、ここでは外付けの BM8563 を使う。
//!
//! **これをカレンダーとしては使わない。** 正しい現在時刻を知る手段
//! (NTP・ユーザー入力)が無いので、初回は 2020-01-01 という固定の基準時刻を
//! 書き込むだけにしてある。記憶が必要とするのは差分(保存してから何秒
//! 経ったか)だけなので、絶対時刻が現実と合っているかは問題にならない。
//! つまり BM8563 は「電源断をまたいで生き残る単調増加カウンタ」として
//! 使っている。将来 WiFi を足して時刻を合わせるなら、この基準時刻を
//! 本物の時刻に置き換えるだけで、上の層は何も変わらない。
//!
//! バックアップ電源が尽きて時刻が失われた場合、BM8563 は秒レジスタの最上位
//! ビットでそれを申告する(`clock_integrity_lost`)。そのときは基準時刻を
//! 書き直す。すると保存済みレコードの時刻は基準より後(=「未来」)になり、
//! `SavedMemory::seconds_away` が 0 を返す —— 経過時間を推測するのではなく
//! 「分からないので減衰させない」を選ぶ。ペットが損をしない側へ倒してある。
//!
//! 日時から Unix 時刻への変換(うるう年の規則)は `vmc_pet_body::civil_time`
//! にある。ここではテストが動かせないため(no_std の Xtensa ターゲット)。

use core_s3::rtc::{Bm8563, Date, DateTime, Time};
use embedded_hal::i2c::I2c;
use vmc_pet_body::unix_seconds_from_civil;

/// 時刻を失っていた RTC に書き込む基準時刻。値そのものに意味はなく、
/// 「ここが起点」という目印。
const BASE_DATETIME: DateTime = DateTime {
    date: Date {
        year: 2020,
        month: 1,
        day: 1,
        weekday: 3, // 2020-01-01 は水曜
    },
    time: Time {
        hour: 0,
        minute: 0,
        second: 0,
    },
};

/// 記憶が要求する形(Unix 時刻)で時計を読む層。
pub struct Clock<I2C> {
    rtc: Bm8563<I2C>,
    /// この起動で基準時刻を書き直したか(= それ以前の時間経過は分からない)。
    lost_its_place: bool,
}

impl<I2C, E> Clock<I2C>
where
    I2C: I2c<Error = E>,
{
    /// RTC を初期化し、時刻を失っていれば基準時刻を書き込む。
    pub fn new(i2c: I2C) -> Result<Self, E> {
        let mut rtc = Bm8563::new(i2c);
        rtc.init()?;
        let lost_its_place = rtc.clock_integrity_lost()?;
        if lost_its_place {
            rtc.set_datetime(BASE_DATETIME)?;
        }
        Ok(Self {
            rtc,
            lost_its_place,
        })
    }

    /// この起動で時計が基準へ巻き戻されたか。起動時のログに出すためにある。
    pub fn lost_its_place(&self) -> bool {
        self.lost_its_place
    }

    /// いまの Unix 時刻(秒)。
    pub fn now_unix_seconds(&mut self) -> Result<u64, E> {
        let datetime = self.rtc.datetime()?;
        Ok(unix_seconds_from_civil(
            datetime.date.year,
            datetime.date.month,
            datetime.date.day,
            datetime.time.hour,
            datetime.time.minute,
            datetime.time.second,
        ))
    }
}
