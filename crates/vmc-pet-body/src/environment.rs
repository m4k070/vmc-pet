//! 「置かれた環境の厳しさ」を決める式。
//!
//! 体を差し替えても変わらない部分をここに置く。プラットフォームごとの読み取り
//! (PC 版は `interface::machine_load` の CPU 負荷、M5Stack 版は AXP2101 の電池の
//! 状態)が違うだけで、**「環境が厳しいほど、エネルギーが早く尽きる」という1つの量**
//! に写すところは同じ。
//!
//! ストレスはエネルギーが尽きる速さだけを変え、安全性には影響しない
//! (`Vitality::apply_environmental_stress` と、体側のテスト
//! `sustained_maximum_stress_still_never_collapses_the_body` 参照)。
//! 成長の強さには実測で見つけた下限(`MIN_GROWTH_SCALE`)があるので、
//! 最大のストレスが続いても体は崩壊しない。
//!
//! 経緯は `docs/M5STACK.md`「『環境ストレス』の意味づけを考え直した」。

/// 電池の残量がこれ以上なら、環境は恵まれている(ストレス 0.0)。
///
/// **実機で見ながら調整する前提の初期値**(docs/DESIGN.md の「表情の加減」と同じ扱い)。
pub const BATTERY_COMFORTABLE_PERCENT: f32 = 60.0;

/// ここまで減ると環境の厳しさが最大(ストレス 1.0)。
///
/// AXP2101 の低電池しきい値(既定 3500mV)がおおよそこの辺りに来る。
pub const BATTERY_STRESSED_PERCENT: f32 = 20.0;

/// 電池の状態から、その体の置かれた環境の厳しさ(0.0..=1.0)を決める。
///
/// - 外部電源につながっている(充電中・満充電を含む) … 0.0。恵まれている
/// - 電池だけで、残量が `BATTERY_COMFORTABLE_PERCENT` 以上 … 0.0
/// - そこから `BATTERY_STRESSED_PERCENT` まで、0.0 → 1.0 に直線で上がる
/// - それ以下 … 1.0
///
/// **低電池のフラグ(1ビット)ではなく連続の残量を使う。** しきい値1つだと
/// 「ある瞬間に急に厳しくなる」階段になり、「電池が減っていくにつれて、体が
/// 少しずつ弱っていく」という読める変化にならない。PC 版が CPU 負荷(連続量)を
/// 使っているのと同じ理由。
///
/// `percent` は 0.0..=100.0 の残量。外れ値が来ても 0.0..=1.0 に収まる。
pub fn stress_from_battery(percent: f32, on_external_power: bool) -> f32 {
    if on_external_power {
        return 0.0;
    }
    let span = BATTERY_COMFORTABLE_PERCENT - BATTERY_STRESSED_PERCENT;
    if span <= 0.0 {
        // 定数を取り違えたときは、厳しさを作らない側に倒す(体を理由なく弱らせない)
        return 0.0;
    }
    ((BATTERY_COMFORTABLE_PERCENT - percent) / span).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plugged_in_device_is_never_a_harsh_environment() {
        // Arrange / Act / Assert: 残量がいくつでも、外部電源があれば 0
        for percent in [0.0, 5.0, 20.0, 50.0, 100.0] {
            assert_eq!(
                stress_from_battery(percent, true),
                0.0,
                "plugged in at {percent}% must be comfortable"
            );
        }
    }

    #[test]
    fn a_comfortably_charged_battery_is_no_stress_at_all() {
        // Arrange / Act / Assert
        assert_eq!(stress_from_battery(100.0, false), 0.0);
        assert_eq!(stress_from_battery(60.0, false), 0.0);
    }

    #[test]
    fn the_stress_ramps_from_comfortable_to_stressed() {
        // Arrange / Act / Assert: 60% と 20% の中間は 0.5
        let middle = stress_from_battery(40.0, false);
        assert!((middle - 0.5).abs() < 1e-6, "got {middle}");
        assert!((stress_from_battery(20.0, false) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn an_empty_battery_cannot_push_the_stress_past_its_range() {
        // Arrange / Act / Assert: 値域外の残量でも 0.0..=1.0 に収まる
        assert_eq!(stress_from_battery(0.0, false), 1.0);
        assert_eq!(stress_from_battery(-30.0, false), 1.0);
        assert_eq!(stress_from_battery(150.0, false), 0.0);
    }

    #[test]
    fn the_stress_never_grows_as_the_battery_recovers() {
        // Arrange: 5% から 100% まで動かす
        let mut previous = 1.1;

        // Act / Assert: 残量が増えてストレスが上がることはない(単調)
        for step in 0..=100 {
            let stress = stress_from_battery(step as f32, false);
            assert!(stress <= previous + 1e-6, "{step}% raised the stress");
            previous = stress;
        }
    }
}
