//! 機械の CPU 負荷を読み取り、体の外側にある「環境の厳しさ」として翻訳する。
//!
//! ユーザーの入力(pointer.rs)と同じ IF層に置くが、ここが翻訳する先は
//! `Perturbation` ではない。CPU 負荷は特定のセルに向けた摂動ではなく、
//! `LeniaBody::apply_environmental_stress` が受け取る周囲環境全体のスカラーだからだ。
//!
//! `/proc/stat` を読むだけで済むため、この用途のためだけに `sysinfo` のような
//! 重い依存を足す必要はない。Linux 専用(このアプリは wlr-layer-shell の時点で
//! 既に Linux に限定されている)。

use std::fs;

/// `/proc/stat` の 1行目(全コア合算)から取り出す値。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CpuTimes {
    /// アイドル(iowait を含む)に費やしたジフィー数。
    idle: u64,
    /// 全カテゴリの合計ジフィー数。
    total: u64,
}

/// CPU 使用率を継続的にサンプリングする。
///
/// `/proc/stat` の値は起動からの累積なので、2回のサンプルの差分から
/// 直近の使用率を求める。1回目の呼び出しは差分が取れないため 0.0 を返す。
pub struct MachineLoad {
    previous: Option<CpuTimes>,
    /// 読み取り失敗のログを一度だけ出すためのフラグ。
    warned: bool,
}

impl MachineLoad {
    pub fn new() -> Self {
        Self {
            previous: None,
            warned: false,
        }
    }

    /// 前回のサンプルからの CPU 使用率(0.0..=1.0)。
    /// `/proc/stat` が読めない環境では、ストレス無し(0.0)として扱う。
    pub fn sample(&mut self) -> f32 {
        let Some(current) = read_cpu_times() else {
            if !self.warned {
                eprintln!("vmc-pet: failed to read /proc/stat; treating cpu load as 0");
                self.warned = true;
            }
            return 0.0;
        };

        let usage = self
            .previous
            .map(|previous| usage_fraction(previous, current))
            .unwrap_or(0.0);
        self.previous = Some(current);
        usage
    }
}

impl Default for MachineLoad {
    fn default() -> Self {
        Self::new()
    }
}

/// `/proc/stat` の先頭行("cpu  ...")を読んで合算値を得る。
fn read_cpu_times() -> Option<CpuTimes> {
    let contents = fs::read_to_string("/proc/stat").ok()?;
    let first_line = contents.lines().next()?;
    let mut fields = first_line.split_whitespace();
    if fields.next()? != "cpu" {
        return None;
    }

    // user, nice, system, idle, iowait, irq, softirq, steal, guest, guest_nice の順。
    let values: Vec<u64> = fields.filter_map(|field| field.parse().ok()).collect();
    let idle = *values.get(3)? + values.get(4).copied().unwrap_or(0);
    let total = values.iter().sum();

    Some(CpuTimes { idle, total })
}

/// 2つのサンプル間で、アイドルでなかった時間の割合を求める。
fn usage_fraction(previous: CpuTimes, current: CpuTimes) -> f32 {
    let total_delta = current.total.saturating_sub(previous.total);
    if total_delta == 0 {
        // 前回とジフィーが変わっていない(サンプリング間隔が短すぎた)場合は
        // 負荷なしとみなす。実際に高負荷でも、次回のサンプルで反映される。
        return 0.0;
    }
    let idle_delta = current.idle.saturating_sub(previous.idle);
    (1.0 - idle_delta as f32 / total_delta as f32).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_fraction_is_zero_when_everything_was_idle() {
        // Arrange
        let previous = CpuTimes { idle: 100, total: 200 };
        let current = CpuTimes { idle: 150, total: 250 };

        // Act
        let usage = usage_fraction(previous, current);

        // Assert: idle も total も 50 増えた = 増分は全部アイドル
        assert_eq!(usage, 0.0);
    }

    #[test]
    fn usage_fraction_is_one_when_nothing_was_idle() {
        // Arrange
        let previous = CpuTimes { idle: 100, total: 200 };
        let current = CpuTimes { idle: 100, total: 250 };

        // Act
        let usage = usage_fraction(previous, current);

        // Assert: total は 50 増えたが idle は増えていない = ずっと働いていた
        assert_eq!(usage, 1.0);
    }

    #[test]
    fn usage_fraction_is_half_when_half_the_new_time_was_busy() {
        // Arrange
        let previous = CpuTimes { idle: 0, total: 0 };
        let current = CpuTimes { idle: 50, total: 100 };

        // Act
        let usage = usage_fraction(previous, current);

        // Assert
        assert!((usage - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn usage_fraction_handles_no_elapsed_time() {
        // Arrange: 2回のサンプルの間にジフィーが進んでいない
        let previous = CpuTimes { idle: 100, total: 200 };
        let current = previous;

        // Act
        let usage = usage_fraction(previous, current);

        // Assert
        assert_eq!(usage, 0.0);
    }

    #[test]
    fn the_first_sample_has_nothing_to_compare_against() {
        // Arrange
        let mut load = MachineLoad::new();

        // Act
        let usage = load.sample();

        // Assert: 実環境の /proc/stat が読めても読めなくても、1回目は必ず 0.0
        assert_eq!(usage, 0.0);
    }

    #[test]
    fn repeated_sampling_of_the_real_proc_stat_stays_within_range() {
        // Arrange
        let mut load = MachineLoad::new();

        // Act / Assert: 実際の /proc/stat を数回読んでも、値域を外れたり
        // パニックしたりしないことを確かめる
        for _ in 0..5 {
            let usage = load.sample();
            assert!((0.0..=1.0).contains(&usage), "usage out of range: {usage}");
        }
    }
}
