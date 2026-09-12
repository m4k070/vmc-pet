//! 西暦の日時を Unix 時刻へ変換する。
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
//! Unix 時刻なので、その間を埋める。
//!
//! `chrono` のようなクレートを足さないのは、要るのがこの一方向の変換だけで、
//! タイムゾーンも書式も一切使わないため。

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
    let seconds =
        days * 86_400 + hour as i64 * 3_600 + minute as i64 * 60 + second as i64;
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
        assert_eq!(unix_seconds_from_civil(2026, 9, 13, 12, 34, 56), 1_789_302_896);
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
}
