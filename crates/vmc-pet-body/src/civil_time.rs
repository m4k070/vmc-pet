//! 西暦の日時と Unix 時刻を相互に変換する。
//!
//! ここにあるのは M5Stack 側だけが使うロジックで、体そのものとは関係がない。
//! それでもこのクレートに置いているのは、**M5Stack 側ではテストが動かせない**
//! ため。あちらは no_std の Xtensa ターゲットで、テストハーネスが無い。
//! うるう年の規則は条件分岐で書くと間違えやすく、間違えれば「停止していた
//! 時間」が丸ごと狂う。同じ理由で `memory::scan_records` もここに置いてある。
//!
//! なぜ必要か: M5Stack には OS も NTP も無いので、停止中の時間を知るには
//! バッテリバックアップ付きの外付け RTC(BM8563)を読むしかなく、それが
//! 返すのはカレンダー上の日時である。記憶(`SavedMemory`)が要求するのは
//! Unix 時刻なので、その間を埋める。逆向き(Unix 時刻 → 日時)は、PC から
//! 受け取った時刻を RTC に書き込むときに使う(`time_sync`)。
//!
//! `chrono` のようなクレートを足さないのは、要るのがこの2つの変換だけで、
//! タイムゾーンも書式も一切使わないため。すべて UTC として扱う。

/// 西暦の日時を Unix 時刻(秒)へ変換する。
///
/// 1970年より前を指していたら 0 を返す。RTC の値が壊れていても、
/// 負の経過時間(= 放置したのにエネルギーが増える)を作らないため。
pub fn unix_seconds_from_civil(
    year: u16,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    second: u8,
) -> u64 {
    let days = days_from_civil(year as i64, month as i64, day as i64);
    let seconds = days * 86_400 + hour as i64 * 3_600 + minute as i64 * 60 + second as i64;
    seconds.max(0) as u64
}

/// 西暦の年月日から、1970-01-01 を 0 とした日数を求める。
///
/// Howard Hinnant の `days_from_civil`(C++ の chrono 設計で使われている
/// もの)を整数演算そのままで書いた。要点は **3月を年の始まりとして扱う**
/// こと。こうするとうるう日が年の最後に来るので、月ごとの日数表も
/// 「2月だけ特別」という分岐も要らなくなる。
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    // 3月始まりに移すため、1月・2月は前の年として数える
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400; // [0, 399]
    let month_from_march = if month > 2 { month - 3 } else { month + 9 }; // [0, 11]
    let day_of_year = (153 * month_from_march + 2) / 5 + day - 1; // [0, 365]
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    // 146097 = 400年ぶんの日数(うるう年の規則がここで一巡する)
    // 719468 = 0000-03-01 から 1970-01-01 までの日数
    era * 146_097 + day_of_era - 719_468
}

/// 1日の秒数。
const SECONDS_PER_DAY: u64 = 86_400;

/// 1970-01-01 の曜日(日曜を 0 とする数え方で木曜)。
const EPOCH_WEEKDAY: u64 = 4;

/// 西暦の日時(UTC)。RTC に書き込む形。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CivilDateTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    /// 曜日。日曜を 0 とする(BM8563 の曜日レジスタと同じ数え方)。
    pub weekday: u8,
}

/// Unix 時刻(秒)を西暦の日時へ変換する(`unix_seconds_from_civil` の逆)。
///
/// 年は `u16` に収まる範囲(西暦65535年まで)を前提にする。それより先を
/// 指す値は受け取る側(RTC は2099年まで)でどのみち扱えないので、呼び出し側が
/// 範囲を確かめてから渡す。
pub fn civil_from_unix_seconds(unix_seconds: u64) -> CivilDateTime {
    let days = unix_seconds / SECONDS_PER_DAY;
    let seconds_into_day = unix_seconds % SECONDS_PER_DAY;
    let (year, month, day) = civil_from_days(days as i64);
    CivilDateTime {
        year: year as u16,
        month: month as u8,
        day: day as u8,
        hour: (seconds_into_day / 3_600) as u8,
        minute: (seconds_into_day / 60 % 60) as u8,
        second: (seconds_into_day % 60) as u8,
        weekday: ((days + EPOCH_WEEKDAY) % 7) as u8,
    }
}

/// 1970-01-01 を 0 とした日数から、西暦の年月日を求める(`days_from_civil` の逆)。
///
/// 同じく Howard Hinnant の `civil_from_days` を整数演算そのままで書いた。
/// 3月始まりの年で数えてから、1月・2月を翌年へ戻す。
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let days = days + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = days - era * 146_097; // [0, 146096]
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365; // [0, 399]
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100); // [0, 365]
    let month_from_march = (5 * day_of_year + 2) / 153; // [0, 11]
    let day = day_of_year - (153 * month_from_march + 2) / 5 + 1; // [1, 31]
    let month = if month_from_march < 10 {
        month_from_march + 3
    } else {
        month_from_march - 9
    }; // [1, 12]
    let year = year_of_era + era * 400 + if month <= 2 { 1 } else { 0 };
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_epoch_itself_is_zero() {
        // Arrange / Act / Assert
        assert_eq!(unix_seconds_from_civil(1970, 1, 1, 0, 0, 0), 0);
    }

    #[test]
    fn the_time_of_day_is_added() {
        // Arrange / Act / Assert
        assert_eq!(unix_seconds_from_civil(1970, 1, 1, 1, 2, 3), 3_723);
    }

    #[test]
    fn a_year_divisible_by_400_is_a_leap_year() {
        // Arrange: 2000-02-29 は存在する
        let leap_day = unix_seconds_from_civil(2000, 2, 29, 0, 0, 0);
        let next_day = unix_seconds_from_civil(2000, 3, 1, 0, 0, 0);

        // Act / Assert: 連続した日になっている
        assert_eq!(next_day - leap_day, 86_400);
    }

    #[test]
    fn a_year_divisible_by_100_but_not_400_is_not_a_leap_year() {
        // Arrange: 2100年は平年なので 2/28 の翌日が 3/1。
        // 1900年で試すと両方とも基準より前で 0 に丸められ、何も検証できない
        let end_of_february = unix_seconds_from_civil(2100, 2, 28, 0, 0, 0);
        let next_day = unix_seconds_from_civil(2100, 3, 1, 0, 0, 0);

        // Act / Assert
        assert_eq!(next_day - end_of_february, 86_400);
    }

    #[test]
    fn known_timestamps_match() {
        // Arrange / Act / Assert: `date -u -d ... +%s` で確かめた既知の値
        // (M5Stack 側が RTC に書き込む基準時刻が 2020-01-01)
        assert_eq!(unix_seconds_from_civil(2020, 1, 1, 0, 0, 0), 1_577_836_800);
        assert_eq!(
            unix_seconds_from_civil(2026, 9, 13, 12, 34, 56),
            1_789_302_896
        );
    }

    #[test]
    fn every_day_of_a_year_advances_by_exactly_one_day() {
        // Arrange: うるう年を含む2年ぶんを1日ずつ辿る。日数表の取り違えは
        // 特定の月境界だけで出るので、全部の境界を通す
        let mut previous = unix_seconds_from_civil(2024, 1, 1, 0, 0, 0);
        let days_in_month = [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

        // Act / Assert
        for (index, days) in days_in_month.into_iter().enumerate() {
            let month = index as u8 + 1;
            for day in 1..=days {
                let current = unix_seconds_from_civil(2024, month, day, 0, 0, 0);
                if (month, day) == (1, 1) {
                    continue;
                }
                assert_eq!(
                    current - previous,
                    86_400,
                    "2024-{month:02}-{day:02} must be one day after the previous date"
                );
                previous = current;
            }
        }
    }

    #[test]
    fn a_date_before_the_epoch_reports_zero_instead_of_going_negative() {
        // Arrange / Act / Assert: RTC の値が壊れていても負にはしない
        assert_eq!(unix_seconds_from_civil(1969, 12, 31, 23, 59, 59), 0);
    }

    #[test]
    fn a_known_timestamp_converts_back_to_its_date_and_weekday() {
        // Arrange: `date -u -d @1789302896` → 2026-09-13 (日) 12:34:56
        let unix_seconds = 1_789_302_896;

        // Act
        let civil = civil_from_unix_seconds(unix_seconds);

        // Assert
        assert_eq!(
            civil,
            CivilDateTime {
                year: 2026,
                month: 9,
                day: 13,
                hour: 12,
                minute: 34,
                second: 56,
                weekday: 0,
            }
        );
    }

    #[test]
    fn weekdays_match_the_rtc_base_time_and_the_epoch() {
        // Arrange / Act / Assert: 1970-01-01 は木曜、2020-01-01 は水曜
        // (clock.rs の基準時刻が weekday 3 としている日)
        assert_eq!(civil_from_unix_seconds(0).weekday, 4);
        assert_eq!(civil_from_unix_seconds(1_577_836_800).weekday, 3);
    }

    #[test]
    fn converting_to_civil_and_back_is_the_identity_across_leap_years_and_centuries() {
        // Arrange: 月末・うるう日・世紀の境目(2100年は平年)を含むよう、
        // 1日と少しずつずらしながら 1999〜2101 年を辿る
        let start = unix_seconds_from_civil(1999, 1, 1, 0, 0, 0);
        let end = unix_seconds_from_civil(2101, 12, 31, 23, 59, 59);
        let stride = SECONDS_PER_DAY + 3_601;

        // Act / Assert
        let mut unix_seconds = start;
        while unix_seconds <= end {
            let civil = civil_from_unix_seconds(unix_seconds);
            let back = unix_seconds_from_civil(
                civil.year,
                civil.month,
                civil.day,
                civil.hour,
                civil.minute,
                civil.second,
            );
            assert_eq!(back, unix_seconds, "round trip failed for {civil:?}");
            unix_seconds += stride;
        }
    }

    #[test]
    fn the_last_second_of_a_leap_day_is_still_february_29() {
        // Arrange / Act
        let civil = civil_from_unix_seconds(unix_seconds_from_civil(2028, 2, 29, 23, 59, 59));

        // Assert
        assert_eq!((civil.month, civil.day, civil.hour), (2, 29, 23));
    }
}
