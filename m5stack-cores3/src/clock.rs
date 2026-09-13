//! 電源が切れている間も進む時計。CoreS3 の BM8563(バッテリバックアップ付き
//! RTC、内部I²Cバス上)を読む。
//!
//! PC版(src/persistence.rs の `now_unix_seconds`)は OS の時計を読むだけで
//! 済むが、M5Stack には OS も NTP も無い。一方で記憶(`SavedMemory`)は
//! 「停止していた秒数」を要求する。`esp_hal` の内蔵 RTC カウンタは電源が
//! 切れると 0 に戻るため、ここでは外付けの BM8563 を使う。
//!
//! **正しい現在時刻は、自分では知らない。** NTP が無いので、時刻を失っていたら
//! 2020-01-01 という固定の基準時刻を書き込む。記憶が必要とする差分(保存してから
//! 何秒経ったか)は、それでも測れる。
//!
//! ただし世話の予測(`vmc_pet_body::care_prediction`)は **1日のうちの何時か** を
//! 学ぶので、時計が実際の時刻とずれていると、PC と同じ時刻が別の「何時」になる。
//! そこで PC から USB シリアルで時刻を送って合わせられるようにした
//! (`set_unix_seconds`、受け取る行の形式は `vmc_pet_body::time_sync`)。
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
use vmc_pet_body::{civil_from_unix_seconds, unix_seconds_from_civil};

/// RTC に書き込める最も早い時刻(2000-01-01T00:00:00Z)。
///
/// BM8563 は年を下2桁で持ち、`core_s3::rtc` は 2000年を起点に読み出すので、
/// 表せるのは 2000〜2099年だけ。
const EARLIEST_SETTABLE_UNIX_SECONDS: u64 = 946_684_800;

/// RTC に書き込める最も遅い時刻(2099-12-31T23:59:59Z)。
const LATEST_SETTABLE_UNIX_SECONDS: u64 = 4_102_444_799;

/// 時計を合わせられなかった理由。
#[derive(Debug)]
pub enum SetTimeError<E> {
    /// RTC が表せない時刻(2000〜2099年の外)。書き込むと別の年として読まれてしまう。
    OutOfRange { unix_seconds: u64 },
    /// RTC との通信に失敗した。
    Rtc(E),
}

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

    /// 時計をこの Unix 時刻(秒、UTC)に合わせる。PC から受け取った時刻を書き込むためにある。
    pub fn set_unix_seconds(&mut self, unix_seconds: u64) -> Result<(), SetTimeError<E>> {
        let settable =
            (EARLIEST_SETTABLE_UNIX_SECONDS..=LATEST_SETTABLE_UNIX_SECONDS).contains(&unix_seconds);
        if !settable {
            return Err(SetTimeError::OutOfRange { unix_seconds });
        }
        let civil = civil_from_unix_seconds(unix_seconds);
        let datetime = DateTime {
            date: Date {
                year: civil.year,
                month: civil.month,
                day: civil.day,
                weekday: civil.weekday,
            },
            time: Time {
                hour: civil.hour,
                minute: civil.minute,
                second: civil.second,
            },
        };
        self.rtc.set_datetime(datetime).map_err(SetTimeError::Rtc)?;
        // 秒レジスタを書き直したので、時刻を失った印(最上位ビット)も消えている
        self.lost_its_place = false;
        Ok(())
    }
}
