//! 世話がいつ来るかを、経験から予測する学習モデル。
//!
//! docs/DESIGN.md のロードマップ「4. 観測から行動への小さな式 → 学習可能なモデル」
//! にあたる。「経験に基づいて変化する生き物」のうち、ここで学ぶのは
//! **その人の生活リズム**(1日のうちのいつ触ってもらえるか)である。
//!
//! # なぜ「予測」なのか
//!
//! 学習の信号をユーザーと相談して決めた。「触られること」を報酬として最大化する
//! 案は、以前採らないと決めている(この一台の履歴だけではデータが足りず、目立とうと
//! 暴れる方向へ偏りやすい)。legibility はシミュレーションで2つの条件を比べる指標で、
//! 実機の1回きりの生活からは計算できない。
//!
//! 予測なら、1分ごとの「触られた/触られなかった」がそのまま学習データになり、
//! 1日に1440件集まる。何かを最大化するわけではないので、暴れる方向へ偏る
//! こともない。予測がどう振る舞いに現れるか(先回りしてそわそわする、など)は
//! このモジュールの外で決める。
//!
//! # モデル
//!
//! 1日のうちの時刻を、sin/cos の3倍音(1日周期・12時間周期・8時間周期)で表した
//! 特徴量に対するロジスティック回帰。重みは7個だけで、何を覚えたかを数値で
//! 読める(このプロジェクトが一貫して採ってきた「なぜ動くかが説明できる」方針)。
//! 3倍音あれば「朝と夜の2回」のような複数の山も表せる。
//!
//! 学習は1件ずつの確率的勾配降下で、学習率は一定にしてある。一定の学習率は
//! 新しい経験ほど重く効く(古い経験が指数的に薄れる)ことと同じなので、
//! 生活リズムが変わればそれに追従する。これが「継続的に学習する」の中身である。
//!
//! # 観測していない時間は学ばない
//!
//! 電源が切れていた間は、触られなかったのではなく**見ていなかった**。そこを
//! 「触られなかった」として学ぶと、電源を切る時間帯ほど世話が来ないと誤って
//! 覚えてしまう。そこで観測に空白があったら、その間は学習せずに集計をやり直す。
//! 時計が巻き戻った場合(RTC の電池切れで基準時刻へ戻った、など)も同じ扱いにする。
//!
//! 時計を読むのはプラットフォーム側で、ここは Unix 時刻を数値として受け取るだけ
//! (`Pet` が時計を知らないという既存の方針と揃えてある)。

use serde::{Deserialize, Serialize};

use crate::math::{cosf, expf, sinf};

const SECONDS_PER_DAY: u64 = 86_400;

/// 集計の単位(秒)。この間に1回でも触られたら「触られた」とする。
const BUCKET_SECONDS: u64 = 60;

/// 時刻を表す sin/cos の倍音の数。
const HARMONICS: usize = 3;

/// 特徴量の数(定数項 + 倍音ごとの sin/cos)。
pub const FEATURES: usize = 1 + 2 * HARMONICS;

/// まだ何も学んでいないときの予測(1分の間に触られる確率)。
///
/// 重みを全部 0 から始めると、どの時刻も 0.5(=2分に1回は触られる)と予測する
/// ことになり、初日から「世話が来るはず」と思い込んだ個体になる。そうならない
/// よう、定数項だけをこの確率に対応する値から始める。
const PRIOR_EXPECTATION: f32 = 0.02;

/// これより長く観測が途切れたら、その間は見ていなかったとみなす。
///
/// 描画・体のステップのたびに観測されるので、動いている間の空白は1秒にも
/// 満たない。集計単位2つぶんを超える空白は、止まっていたとしか考えられない。
const MAX_OBSERVATION_GAP_SECONDS: u64 = 2 * BUCKET_SECONDS;

/// 学習の設定。学習率を計測で選べるよう、定数ではなく構造体にしてある
/// (`ControllerParams` と同じ扱い)。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CarePredictorParams {
    /// 1件ごとの勾配降下の歩幅。大きいほど新しい経験に早く追従するが、
    /// 日々のばらつきに振り回されやすくなる。
    pub learning_rate: f32,
}

impl Default for CarePredictorParams {
    /// 学習率 0.02 は計測で選んだ(docs/DESIGN.md「世話の予測」)。
    ///
    /// 合成したユーザー(14日間は毎晩20時台に触る)で比べると、0.05 以上では
    /// **たまたま朝に触った日が1日あるだけで、2週間ぶんの「夜に来る」をほぼ
    /// 打ち消した**(夜/朝の予測比 263 → 0.8)。0.02 なら同じ日を経ても夜を
    /// 予測し続け(比 5.7)、2日で元の確信へ戻る。一方で習慣が本当に変われば
    /// 2日で追従する。0.01 は例外にはさらに強いが、本当の変化に3日かかる。
    fn default() -> Self {
        Self {
            learning_rate: 0.02,
        }
    }
}

/// 再起動をまたいで持ち越す、学んだことの中身(`PetMemory` に含めて保存する)。
///
/// 生活リズムは何日もかけて覚えるものなので、これを保存しないと再起動のたびに
/// 何も知らない個体に戻り、継続的な学習として意味をなさない。持ち越すのは重み
/// だけで、集計途中の1分は持ち越さない(止まっていた間は見ていなかった)。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CareMemory {
    /// 予測モデルの重み(定数項と、倍音ごとの sin/cos)。
    pub weights: [f32; FEATURES],
}

impl Default for CareMemory {
    /// 何も学んでいない状態。定数項だけを事前確率に対応する値にしてある。
    fn default() -> Self {
        let mut weights = [0.0; FEATURES];
        weights[0] = logit(PRIOR_EXPECTATION);
        Self { weights }
    }
}

/// 1日の平均の予測を求めるときに、1日を何点で標本化するか(15分おき)。
const SAMPLES_PER_DAY: u64 = 96;

/// 予測がその日の平均の何倍になったら、期待(`anticipation_at`)を最大にするか。
///
/// 倍率で見るのは、触られる頻度そのものが人によって大きく違うため。まれにしか
/// 触らない人でも「この時間は他より来やすい」ことは学べるので、絶対値ではなく
/// その個体が覚えた1日の中での相対的な山を期待にする。
const FULL_ANTICIPATION_RATIO: f32 = 3.0;

/// 予測がその日の平均の何倍を超えたら、期待し始めるか。
///
/// ほぼ平均並みの時間帯は特別な時間ではないので期待しない。これが無いと、
/// 何も学んでいない(1日じゅう平らな)個体でも、平均を求める足し算の丸め誤差で
/// 比がわずかに 1 を超え、ごく小さな期待が生じていた(テストで見つかった)。
const ANTICIPATION_STARTS_AT_RATIO: f32 = 1.1;

/// 期待し始めるのに要る、予測の最低限の高さ(何も学んでいないときの予測の1.5倍)。
///
/// その日の平均との比だけで期待を決めていたら、**一度も触られていない個体にも
/// 期待が生じた**(テストで発覚)。触られなかった1分から学ぶたびに倍音の重みが
/// 少しずつ動き、平らだった予測にごく小さな凹凸ができる。値そのものは事前確率の
/// ままほとんど変わらないのに、平均との比では凹凸が山に見えてしまっていた。
/// 期待には「その時間帯に実際に触られた証拠」が要る、という条件を足した。
///
/// 代償として、ごくまれにしか触らない人(その時間帯でも1分あたりの確率が3%未満)
/// には期待を形成しない。
const MIN_EXPECTATION_TO_ANTICIPATE: f32 = PRIOR_EXPECTATION * 1.5;

/// 期待しているのに世話が来ない状態が、何分続くとがっかりが最大になるか
/// (期待が 1.0 のとき)。手触りで調整する前提の初期値。
const MINUTES_OF_UNMET_WAITING_FOR_FULL_DISAPPOINTMENT: f32 = 30.0;

/// がっかりが1分ごとに薄れる割合。半減期がおよそ90分になる値(0.5^(1/90))。
/// いつもの時間を外されても、翌朝にはほぼ消えている(手触りで調整する前提)。
const DISAPPOINTMENT_FADE_PER_MINUTE: f32 = 0.992_33;

/// 世話がいつ来るかの予測モデル。
#[derive(Debug, Clone)]
pub struct CarePredictor {
    params: CarePredictorParams,
    weights: [f32; FEATURES],
    /// 1日を通した予測の平均。重みが変わる(1分に1回)ときだけ計算し直す。
    /// `anticipation_at` は体のステップごとに呼ばれうるので、そのたびに
    /// 1日ぶんの三角関数を計算しないようにするため(M5Stack で効く)。
    daily_mean: f32,
    /// 期待していたのに世話が来なかったことの溜まり具合(0.0..=1.0)。
    disappointment: f32,
    /// いまの期待がすでに満たされたか(その時間帯に一度触られたか)。
    /// 満たされた後は、同じ時間帯が続いてもがっかりを溜めない。期待が
    /// 引いた(0 になった)ところで解除する。
    expectation_fulfilled: bool,
    /// いま集計している単位の開始時刻。まだ何も観測していなければ `None`。
    bucket_start: Option<u64>,
    /// いまの集計単位の間に触られたか。
    touched_in_bucket: bool,
}

impl CarePredictor {
    pub fn new() -> Self {
        Self::with_params(CarePredictorParams::default())
    }

    pub fn with_params(params: CarePredictorParams) -> Self {
        let mut predictor = Self {
            params,
            weights: CareMemory::default().weights,
            daily_mean: PRIOR_EXPECTATION,
            disappointment: 0.0,
            expectation_fulfilled: false,
            bucket_start: None,
            touched_in_bucket: false,
        };
        predictor.daily_mean = predictor.mean_expectation_over_day();
        predictor
    }

    /// その時刻の1分の間に触られる確率の予測(0.0..1.0)。
    pub fn expectation_at(&self, unix_seconds: u64) -> f32 {
        sigmoid(dot(&self.weights, &features(unix_seconds)))
    }

    /// その時刻に、世話が来ることをどれだけ期待しているか(0.0..=1.0)。
    ///
    /// 予測がその日の平均の `ANTICIPATION_STARTS_AT_RATIO` 倍以下なら 0.0、
    /// `FULL_ANTICIPATION_RATIO` 倍以上なら 1.0。何も学んでいない個体は予測が
    /// 1日じゅう平らなので、どの時刻も 0.0 になる(根拠のない期待をしない)。
    ///
    /// 世話が来る少し前から期待が高まるのは、3倍音の予測が山を前後になだらかに
    /// 広げるため。「何分前から」を別に持たなくても、自然に先回りになる。
    pub fn anticipation_at(&self, unix_seconds: u64) -> f32 {
        let expectation = self.expectation_at(unix_seconds);
        if expectation < MIN_EXPECTATION_TO_ANTICIPATE {
            return 0.0;
        }
        let ratio = expectation / self.daily_mean;
        let above_ordinary = ratio - ANTICIPATION_STARTS_AT_RATIO;
        let span = FULL_ANTICIPATION_RATIO - ANTICIPATION_STARTS_AT_RATIO;
        (above_ordinary / span).clamp(0.0, 1.0)
    }

    /// いまの時刻を知らせる。描画や体のステップのたびに呼んでよい。
    ///
    /// 集計単位が切り替わっていたら、終わった単位を1件の経験として学ぶ。
    pub fn observe(&mut self, now_unix_seconds: u64) {
        let bucket = now_unix_seconds - now_unix_seconds % BUCKET_SECONDS;
        let Some(start) = self.bucket_start else {
            self.start_bucket(bucket);
            return;
        };
        if bucket == start {
            return;
        }
        // 時計の巻き戻り、または長い空白(止まっていた)。見ていなかった時間は学ばない。
        let observed_continuously =
            bucket > start && bucket - start <= MAX_OBSERVATION_GAP_SECONDS;
        if !observed_continuously {
            self.start_bucket(bucket);
            return;
        }

        self.learn(start, self.touched_in_bucket);
        // 空白が短く、途中の単位を丸ごと飛ばした場合は、そこは触られなかったとして学ぶ
        let mut skipped = start + BUCKET_SECONDS;
        while skipped < bucket {
            self.learn(skipped, false);
            skipped += BUCKET_SECONDS;
        }
        self.start_bucket(bucket);
    }

    /// 触られた(クリックされた)ことを知らせる。
    pub fn record_touch(&mut self, now_unix_seconds: u64) {
        self.observe(now_unix_seconds);
        self.touched_in_bucket = true;
    }

    /// 学んだ重み。何を覚えたかを読むため、また保存するためにある。
    pub fn weights(&self) -> [f32; FEATURES] {
        self.weights
    }

    /// 持ち越すべき学んだことを取り出す(保存用)。
    pub fn memory(&self) -> CareMemory {
        CareMemory {
            weights: self.weights,
        }
    }

    /// 保存しておいた学んだことから再開する。
    ///
    /// 有限でない値が混ざっていたら(壊れた保存データ)、何も学んでいない状態から
    /// 始める。NaN の重みは予測を NaN にし、期待を通じて振る舞いまで汚染するため。
    /// 集計途中の1分は持ち越さない。
    pub fn restore(&mut self, memory: CareMemory) {
        let intact = memory.weights.iter().all(|weight| weight.is_finite());
        self.weights = if intact {
            memory.weights
        } else {
            CareMemory::default().weights
        };
        self.daily_mean = self.mean_expectation_over_day();
        self.disappointment = 0.0;
        self.expectation_fulfilled = false;
        self.bucket_start = None;
        self.touched_in_bucket = false;
    }

    /// 期待していたのに世話が来なかったことの溜まり具合(0.0..=1.0)。
    ///
    /// 期待(`anticipation_at`)が学習の**前向きの**予測なら、がっかりは
    /// その予測の**外れ**の記録である。学習の信号と同じ誤差から生まれる感情、
    /// という位置づけにしてある(docs/DESIGN.md「元気/待っている/がっかり」)。
    ///
    /// - 期待している時間に触られなかった1分ごとに、期待の大きさに応じて溜まる
    /// - 触られたらその場で0に戻り、その時間帯のうちはもう溜まらない
    /// - 1分ごとに少しずつ薄れる(半減期およそ90分)
    /// - 観測していなかった時間(電源断・時計の巻き戻り)は溜まらない
    ///
    /// 保存はしない。数時間で薄れるので、次に起動するまでにはどうせ消えている
    /// (慣れを保存しないのと同じ判断)。
    pub fn disappointment(&self) -> f32 {
        self.disappointment
    }

    /// 1分ぶんの経験から、がっかりを更新する。
    fn update_disappointment(&mut self, anticipation: f32, touched: bool) {
        if touched {
            self.disappointment = 0.0;
            self.expectation_fulfilled = true;
            return;
        }
        if anticipation <= 0.0 {
            self.expectation_fulfilled = false;
        }
        let unmet = if self.expectation_fulfilled { 0.0 } else { anticipation };
        let accumulated = self.disappointment * DISAPPOINTMENT_FADE_PER_MINUTE
            + unmet / MINUTES_OF_UNMET_WAITING_FOR_FULL_DISAPPOINTMENT;
        self.disappointment = accumulated.min(1.0);
    }

    fn start_bucket(&mut self, bucket: u64) {
        self.bucket_start = Some(bucket);
        self.touched_in_bucket = false;
    }

    /// 1件の経験から学ぶ(ロジスティック回帰の確率的勾配降下)。
    fn learn(&mut self, bucket_start: u64, touched: bool) {
        let middle = bucket_start + BUCKET_SECONDS / 2;
        // 「その1分を期待していたか」は、この経験で学ぶ前の予測で判断する
        self.update_disappointment(self.anticipation_at(middle), touched);
        // 集計単位の中央の時刻で特徴量を作る
        let x = features(middle);
        let predicted = sigmoid(dot(&self.weights, &x));
        let observed = if touched { 1.0 } else { 0.0 };
        let error = observed - predicted;
        for (weight, feature) in self.weights.iter_mut().zip(x.iter()) {
            *weight += self.params.learning_rate * error * feature;
        }
        self.daily_mean = self.mean_expectation_over_day();
    }

    /// 1日を通した予測の平均。
    fn mean_expectation_over_day(&self) -> f32 {
        let step = SECONDS_PER_DAY / SAMPLES_PER_DAY;
        let total: f32 = (0..SAMPLES_PER_DAY)
            .map(|sample| self.expectation_at(sample * step))
            .sum();
        total / SAMPLES_PER_DAY as f32
    }
}

impl Default for CarePredictor {
    fn default() -> Self {
        Self::new()
    }
}

/// 1日のうちの時刻を、定数項と sin/cos の倍音で表す。
fn features(unix_seconds: u64) -> [f32; FEATURES] {
    let seconds_into_day = (unix_seconds % SECONDS_PER_DAY) as f32;
    let phase = core::f32::consts::TAU * seconds_into_day / SECONDS_PER_DAY as f32;
    let mut x = [0.0; FEATURES];
    x[0] = 1.0;
    for harmonic in 1..=HARMONICS {
        let angle = phase * harmonic as f32;
        x[2 * harmonic - 1] = sinf(angle);
        x[2 * harmonic] = cosf(angle);
    }
    x
}

fn dot(weights: &[f32; FEATURES], x: &[f32; FEATURES]) -> f32 {
    weights.iter().zip(x.iter()).map(|(w, v)| w * v).sum()
}

/// 数値的に安定なロジスティック関数(大きな負の入力で exp が溢れないようにする)。
fn sigmoid(z: f32) -> f32 {
    if z >= 0.0 {
        1.0 / (1.0 + expf(-z))
    } else {
        let e = expf(z);
        e / (1.0 + e)
    }
}

/// `sigmoid` の逆関数。確率から、それを予測する定数項を求める。
fn logit(probability: f32) -> f32 {
    crate::math::lnf(probability / (1.0 - probability))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ある日の0時(UTC)。
    const MIDNIGHT: u64 = 20_000 * SECONDS_PER_DAY;

    fn at(hour: u64, minute: u64) -> u64 {
        MIDNIGHT + hour * 3_600 + minute * 60
    }

    #[test]
    fn a_fresh_predictor_expects_little_care_and_the_same_at_every_hour() {
        // Arrange / Act
        let predictor = CarePredictor::new();

        // Assert: どの時刻も同じ低い予測から始まる
        for hour in 0..24 {
            let expectation = predictor.expectation_at(at(hour, 0));
            assert!(
                (expectation - PRIOR_EXPECTATION).abs() < 1e-4,
                "hour {hour}: got {expectation}"
            );
        }
    }

    #[test]
    fn a_minute_with_a_touch_raises_the_expectation_at_that_time() {
        // Arrange
        let mut predictor = CarePredictor::new();
        let before = predictor.expectation_at(at(20, 30));

        // Act: 20:30 の1分間に触り、次の1分へ進めて学ばせる
        predictor.observe(at(20, 30));
        predictor.record_touch(at(20, 30) + 10);
        predictor.observe(at(20, 31));

        // Assert
        assert!(predictor.expectation_at(at(20, 30)) > before);
    }

    #[test]
    fn nothing_is_learned_until_the_minute_is_over() {
        // Arrange
        let mut predictor = CarePredictor::new();
        let before = predictor.weights();

        // Act: 同じ1分の中で触って時間を進めるだけ
        predictor.observe(at(20, 30));
        predictor.record_touch(at(20, 30) + 10);
        predictor.observe(at(20, 30) + 50);

        // Assert
        assert_eq!(predictor.weights(), before);
    }

    #[test]
    fn many_touches_in_one_minute_count_as_one() {
        // Arrange
        let mut once = CarePredictor::new();
        let mut many = CarePredictor::new();

        // Act
        once.record_touch(at(20, 30));
        once.observe(at(20, 31));
        for second in 0..30 {
            many.record_touch(at(20, 30) + second);
        }
        many.observe(at(20, 31));

        // Assert: 連打しても1回の世話としか数えない
        assert_eq!(once.weights(), many.weights());
    }

    #[test]
    fn time_while_switched_off_is_not_learned_as_neglect() {
        // Arrange: 観測を始める
        let mut predictor = CarePredictor::new();
        predictor.observe(at(8, 0));
        let before = predictor.weights();

        // Act: 8時間止まっていて、再び観測する
        predictor.observe(at(16, 0));

        // Assert: 見ていなかった8時間ぶんを「触られなかった」として学んではいない
        assert_eq!(predictor.weights(), before);
    }

    #[test]
    fn a_clock_that_went_backwards_is_not_learned_from() {
        // Arrange
        let mut predictor = CarePredictor::new();
        predictor.observe(at(20, 30));
        predictor.record_touch(at(20, 30));
        let before = predictor.weights();

        // Act: 時計が昼へ巻き戻る(RTC が基準時刻に戻った、など)
        predictor.observe(at(12, 0));

        // Assert: 巻き戻った時刻で学ばない。そのときの触れた記録も捨てる
        assert_eq!(predictor.weights(), before);
        predictor.observe(at(12, 1));
        assert!(
            predictor.expectation_at(at(12, 0)) < PRIOR_EXPECTATION,
            "the touch before the clock jumped must not be credited to the new time"
        );
    }

    /// テスト専用の決定的な疑似乱数(xorshift64)。
    struct Rng(u64);

    impl Rng {
        fn unit(&mut self) -> f32 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            (x >> 11) as f32 / (1u64 << 53) as f32
        }
    }

    /// 合成したユーザーと `days` 日暮らす。`habit_hour(day)` の時間帯は1分ごとに
    /// 確率0.5で、それ以外はまれに(0.005)触る。
    fn live(predictor: &mut CarePredictor, days: u64, habit_hour: impl Fn(u64) -> u64) {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        for day in 0..days {
            for minute in 0..1_440 {
                let now = MIDNIGHT + day * SECONDS_PER_DAY + minute * 60;
                let probability = if minute / 60 == habit_hour(day) { 0.5 } else { 0.005 };
                predictor.observe(now);
                if rng.unit() < probability {
                    predictor.record_touch(now + 5);
                }
            }
        }
    }

    fn evening_over_morning(predictor: &CarePredictor) -> f32 {
        predictor.expectation_at(at(20, 30)) / predictor.expectation_at(at(8, 30))
    }

    #[test]
    fn a_week_of_evening_visits_is_learned_as_an_evening_habit() {
        // Arrange
        let mut predictor = CarePredictor::new();

        // Act: 1週間、毎晩20時台に触る
        live(&mut predictor, 7, |_| 20);

        // Assert: 夜を朝よりずっと強く予測する(計測では約66倍)
        let ratio = evening_over_morning(&predictor);
        assert!(ratio > 20.0, "got evening/morning = {ratio}");
    }

    #[test]
    fn a_single_unusual_day_does_not_overturn_a_habit() {
        // Arrange: 2週間の夜の習慣
        let mut predictor = CarePredictor::new();

        // Act: 15日目だけ朝に触り、また夜に戻る
        live(&mut predictor, 15, |day| if day == 14 { 8 } else { 20 });
        let right_after = evening_over_morning(&predictor);
        let mut recovering = CarePredictor::new();
        live(&mut recovering, 17, |day| if day == 14 { 8 } else { 20 });
        let two_days_later = evening_over_morning(&recovering);

        // Assert: 例外の1日を経ても夜を予測し続け(計測では約5.7倍)、
        // 2日で確信を取り戻す(約24倍)。学習率を上げすぎるとここが崩れる
        assert!(right_after > 2.0, "one odd day must not flip the habit; got {right_after}");
        assert!(two_days_later > 10.0, "the habit must come back; got {two_days_later}");
    }

    #[test]
    fn a_habit_that_really_changes_is_followed_within_a_few_days() {
        // Arrange
        let mut predictor = CarePredictor::new();

        // Act: 2週間夜に触ったあと、3日間朝に触る
        live(&mut predictor, 17, |day| if day < 14 { 20 } else { 8 });

        // Assert: 朝を夜より強く予測するようになっている
        let ratio = evening_over_morning(&predictor);
        assert!(ratio < 1.0, "a lasting change must be followed; got evening/morning = {ratio}");
    }

    #[test]
    fn a_fresh_predictor_anticipates_nothing() {
        // Arrange / Act
        let predictor = CarePredictor::new();

        // Assert: 学んでいないのに、特定の時間に期待したりしない
        for hour in 0..24 {
            assert_eq!(predictor.anticipation_at(at(hour, 0)), 0.0, "hour {hour}");
        }
    }

    #[test]
    fn a_learned_evening_habit_is_anticipated_in_the_evening_only() {
        // Arrange
        let mut predictor = CarePredictor::new();

        // Act: 1週間、毎晩20時台に触る
        live(&mut predictor, 7, |_| 20);

        // Assert: 夜は期待し、朝は期待しない
        let evening = predictor.anticipation_at(at(20, 30));
        let morning = predictor.anticipation_at(at(8, 30));
        assert!(evening > 0.9, "got {evening}");
        assert_eq!(morning, 0.0);
    }

    #[test]
    fn anticipation_builds_up_before_the_usual_time() {
        // Arrange
        let mut predictor = CarePredictor::new();
        live(&mut predictor, 7, |_| 20);

        // Act
        let two_hours_before = predictor.anticipation_at(at(18, 0));
        let one_hour_before = predictor.anticipation_at(at(19, 0));

        // Assert: いつもの時間に向けて、前から期待が高まっていく
        assert!(
            one_hour_before > two_hours_before && one_hour_before > 0.0,
            "18:00={two_hours_before} 19:00={one_hour_before}"
        );
    }

    #[test]
    fn the_cached_daily_mean_matches_a_minute_by_minute_average() {
        // Arrange: 偏りのある山を学ばせる
        let mut predictor = CarePredictor::new();
        live(&mut predictor, 7, |_| 20);

        // Act: 1分ごとに平均を取り直す
        let fine: f32 = (0..1_440).map(|minute| predictor.expectation_at(minute * 60)).sum::<f32>()
            / 1_440.0;

        // Assert: 15分おきの標本で求めたキャッシュと、ほぼ一致する
        let relative_error = (predictor.daily_mean - fine).abs() / fine;
        assert!(relative_error < 0.01, "cached={} fine={fine}", predictor.daily_mean);
    }

    #[test]
    fn a_restored_predictor_remembers_the_habit_it_learned() {
        // Arrange: 1週間ぶんの夜の習慣を学ばせてから保存する
        let mut learned = CarePredictor::new();
        live(&mut learned, 7, |_| 20);
        let memory = learned.memory();

        // Act: 何も知らない個体に復元する
        let mut reborn = CarePredictor::new();
        reborn.restore(memory);

        // Assert: 同じ予測・同じ期待を持つ
        assert_eq!(reborn.weights(), learned.weights());
        assert_eq!(reborn.anticipation_at(at(20, 30)), learned.anticipation_at(at(20, 30)));
    }

    #[test]
    fn a_corrupted_memory_starts_over_instead_of_poisoning_the_predictions() {
        // Arrange: NaN の混ざった保存データ
        let mut weights = CareMemory::default().weights;
        weights[3] = f32::NAN;

        // Act
        let mut predictor = CarePredictor::new();
        predictor.restore(CareMemory { weights });

        // Assert: 何も学んでいない状態から始まり、予測は確率のまま
        assert_eq!(predictor.memory(), CareMemory::default());
        assert!(predictor.expectation_at(at(20, 30)).is_finite());
    }

    /// 1週間ぶん夜の習慣を学ばせた予測モデル。
    fn with_an_evening_habit() -> CarePredictor {
        let mut predictor = CarePredictor::new();
        live(&mut predictor, 7, |_| 20);
        predictor
    }

    /// 8日目の `from` から `until` まで、1分ごとに観測する(触られない)。
    fn wait_unvisited(predictor: &mut CarePredictor, from: (u64, u64), until: (u64, u64)) {
        let day8 = 7 * SECONDS_PER_DAY;
        let mut now = at(from.0, from.1) + day8;
        while now <= at(until.0, until.1) + day8 {
            predictor.observe(now);
            now += 60;
        }
    }

    #[test]
    fn a_pet_that_is_never_visited_never_starts_expecting_anyone() {
        // Arrange
        let mut predictor = CarePredictor::new();

        // Act: 3日間、一度も誰も来ない。触られなかった経験から学び続ける
        for minute in 0..(3 * 1_440) {
            predictor.observe(MIDNIGHT + minute * 60);
            // Assert: その間どの時刻にも、来ない相手を待ったりがっかりしたりしない。
            // 平均との比だけで期待を決めていた頃は、学習でできた小さな凹凸が
            // 山に見えて、ここで期待とがっかりが生じていた
            assert_eq!(predictor.anticipation_at(MIDNIGHT + minute * 60), 0.0, "minute {minute}");
            assert_eq!(predictor.disappointment(), 0.0, "minute {minute}");
        }
    }

    #[test]
    fn waiting_in_vain_through_the_usual_time_leaves_the_pet_disappointed() {
        // Arrange
        let mut predictor = with_an_evening_habit();

        // Act: 8日目、いつもの夜の時間を過ぎても誰も来ない
        wait_unvisited(&mut predictor, (0, 0), (21, 30));

        // Assert
        let disappointment = predictor.disappointment();
        assert!(disappointment > 0.8, "got {disappointment}");
    }

    #[test]
    fn a_visit_at_the_usual_time_leaves_no_disappointment() {
        // Arrange
        let mut predictor = with_an_evening_habit();
        let day8 = 7 * SECONDS_PER_DAY;

        // Act: 8日目、20:30 に来てくれて、そのまま夜が過ぎる
        wait_unvisited(&mut predictor, (0, 0), (20, 29));
        predictor.record_touch(at(20, 30) + day8);
        wait_unvisited(&mut predictor, (20, 31), (21, 30));

        // Assert: 期待が満たされたので、その後の時間帯でもがっかりは溜まらない
        let disappointment = predictor.disappointment();
        assert!(disappointment < 0.05, "got {disappointment}");
    }

    #[test]
    fn disappointment_fades_by_the_next_morning() {
        // Arrange: 夜の時間を外された
        let mut predictor = with_an_evening_habit();
        wait_unvisited(&mut predictor, (0, 0), (21, 30));

        // Act: 翌朝8時まで、誰も来ないまま時間が過ぎる
        let day9 = 8 * SECONDS_PER_DAY;
        let mut now = at(21, 31) + 7 * SECONDS_PER_DAY;
        while now <= at(8, 0) + day9 {
            predictor.observe(now);
            now += 60;
        }

        // Assert
        let disappointment = predictor.disappointment();
        assert!(disappointment < 0.05, "got {disappointment}");
    }

    #[test]
    fn being_switched_off_through_the_usual_time_is_not_disappointment() {
        // Arrange
        let mut predictor = with_an_evening_habit();

        // Act: 8日目、夕方に電源を切り、夜遅くに入れる
        wait_unvisited(&mut predictor, (0, 0), (17, 0));
        wait_unvisited(&mut predictor, (23, 0), (23, 1));

        // Assert: 見ていなかった時間の「来なかった」はがっかりにならない
        let disappointment = predictor.disappointment();
        assert!(disappointment < 0.1, "got {disappointment}");
    }

    #[test]
    fn the_expectation_is_always_a_probability() {
        // Arrange: 極端に偏った経験を大量に積む
        let mut predictor = CarePredictor::with_params(CarePredictorParams {
            learning_rate: 1.0,
        });

        // Act
        for minute in 0..(10 * 1_440) {
            let now = MIDNIGHT + minute * 60;
            predictor.record_touch(now);
        }

        // Assert
        for hour in 0..24 {
            let expectation = predictor.expectation_at(at(hour, 0));
            assert!(
                (0.0..=1.0).contains(&expectation) && expectation.is_finite(),
                "hour {hour}: got {expectation}"
            );
        }
    }
}
