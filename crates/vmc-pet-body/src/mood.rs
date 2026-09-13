//! 気分: 世話がいつ来るかの学習と、そこから生まれる期待・がっかり、そしてそれを
//! 体の表情(テンポ・色素)へ伝える変換。
//!
//! 以前はこれらが `Pet` のフィールドとして散らばっていた(予測モデル、期待、がっかり、
//! 最後に知らされた時刻、がっかりをテンポへ変える式)。`Pet` は体・echo・色素・慣れ・
//! コントローラを束ねる役なので、気分というひとまとまりをここに切り出し、`Pet` からは
//! 「体をどのテンポで進めるか」「色素にどれだけ刺激を与えるか」だけを尋ねる形にした。
//!
//! 時計は読まない。Unix 時刻は呼び出し側(`Pet::tick_clock` を呼ぶプラットフォーム)から
//! 受け取る(`Pet` が時計を知らないという既存の方針と揃えてある)。

use crate::{CareMemory, CarePredictor};

/// がっかりしきったときの体のテンポ(1.0 がいつもどおり)。
///
/// 3状態をテンポに割り当てた計測(docs/DESIGN.md「3状態をテンポに割り当てて直接測った」)
/// で、×0.6 は全生物で崩壊せず、コントローラでは見分けられなかった OG2g・2S1v でも
/// 「待っている⇔がっかり」が 0.36 前後になった。がっかりの溜まり具合に比例して
/// 遅くなり、がっかりが薄れればいつものテンポに戻る。
const DISAPPOINTED_TEMPO: f32 = 0.6;

/// 見た目で見分けられるようにしたい気分の状態(docs/DESIGN.md「元気/待っている/がっかり」)。
///
/// 評価(`fitness::mood_trajectory`)と、見た目を確かめるためのプレビュー(PC の
/// `--preview-mood`、M5Stack の `VMC_PET_PREVIEW_MOOD`)で共有する。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoodState {
    /// 世話され続けていて元気。世話を期待してもいない。
    Lively,
    /// いつもの時間なので世話を待っている。
    Waiting,
    /// いつもの時間に来てもらえずがっかりしている。
    Disappointed,
}

impl MoodState {
    pub const ALL: [MoodState; 3] = [
        MoodState::Lively,
        MoodState::Waiting,
        MoodState::Disappointed,
    ];

    /// 人に見せる名前。
    pub fn label(self) -> &'static str {
        match self {
            MoodState::Lively => "元気",
            MoodState::Waiting => "待っている",
            MoodState::Disappointed => "がっかり",
        }
    }

    /// コマンドラインやビルド時の指定に使う名前。
    pub fn name(self) -> &'static str {
        match self {
            MoodState::Lively => "lively",
            MoodState::Waiting => "waiting",
            MoodState::Disappointed => "disappointed",
        }
    }

    /// `name` で指定された状態。知らない名前なら `None`。
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|state| state.name() == name)
    }

    /// この状態に固定するときの期待とがっかり。
    pub fn anticipation_and_disappointment(self) -> (f32, f32) {
        match self {
            MoodState::Lively => (0.0, 0.0),
            MoodState::Waiting => (1.0, 0.0),
            MoodState::Disappointed => (0.0, 1.0),
        }
    }
}

/// 気分。
#[derive(Debug, Clone)]
pub struct Mood {
    /// 世話がいつ来るかを経験から学ぶ予測モデル。
    care: CarePredictor,
    /// いま世話が来ることをどれだけ期待しているか(0.0..=1.0)。`tick_clock` で更新する。
    /// 時計を渡されない限り 0.0 のままなので、評価やテストでは振る舞いが変わらない。
    anticipation: f32,
    /// 期待していたのに世話が来なかったことの溜まり具合(0.0..=1.0)。
    /// `tick_clock` で更新する。時計を渡されない限り 0.0 のまま。
    disappointment: f32,
    /// 期待とがっかりが固定されているか(`pin`)。固定中は `tick_clock` で変わらない。
    pinned: bool,
    /// 最後に知らされた時刻(Unix 秒)。世話が来たことを予測モデルに記録するのに使う。
    now_unix_seconds: Option<u64>,
}

impl Mood {
    pub fn new() -> Self {
        Self {
            care: CarePredictor::new(),
            anticipation: 0.0,
            disappointment: 0.0,
            pinned: false,
            now_unix_seconds: None,
        }
    }

    /// いまの時刻(Unix 秒)を知らせる。学習が進み、期待とがっかりが更新される
    /// (固定されていれば、学習だけが進み値は変わらない)。
    pub fn tick_clock(&mut self, now_unix_seconds: u64) {
        self.now_unix_seconds = Some(now_unix_seconds);
        self.care.observe(now_unix_seconds);
        if self.pinned {
            return;
        }
        self.anticipation = self.care.anticipation_at(now_unix_seconds);
        self.disappointment = self.care.disappointment();
    }

    /// 世話が来た(クリックされた)ことを知らせる。
    ///
    /// まだ時刻を知らされていなければ、いつ来たか分からないので何も覚えない。
    pub fn record_visit(&mut self) {
        if let Some(now) = self.now_unix_seconds {
            self.care.record_touch(now);
        }
    }

    /// いま世話が来ることをどれだけ期待しているか(0.0..=1.0)。
    pub fn anticipation(&self) -> f32 {
        self.anticipation
    }

    /// 期待していたのに世話が来なかったことの溜まり具合(0.0..=1.0)。
    pub fn disappointment(&self) -> f32 {
        self.disappointment
    }

    /// 体を進めるテンポ(1.0 がいつもどおり)。がっかりしているほどゆっくりになる。
    pub fn tempo(&self) -> f32 {
        let disappointment = self.disappointment.clamp(0.0, 1.0);
        1.0 - (1.0 - DISAPPOINTED_TEMPO) * disappointment
    }

    /// 体に付く色素へ与える刺激(0.0..=1.0)。世話を待っているほど色づく。
    pub fn pigment_stimulus(&self) -> f32 {
        self.anticipation
    }

    /// 持ち越すべき学んだことを取り出す(保存用)。
    pub fn memory(&self) -> CareMemory {
        self.care.memory()
    }

    /// 保存しておいた学んだことから再開する。
    pub fn restore(&mut self, memory: CareMemory) {
        self.care.restore(memory);
    }

    /// 期待とがっかりを固定する(`Pet::pin_mood` 参照)。何度呼んでもよく、最後の値が残る。
    pub fn pin(&mut self, anticipation: f32, disappointment: f32) {
        self.pinned = true;
        self.anticipation = anticipation.clamp(0.0, 1.0);
        self.disappointment = disappointment.clamp(0.0, 1.0);
    }
}

impl Default for Mood {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_mood_leaves_the_body_as_it_is() {
        // Arrange / Act
        let mood = Mood::new();

        // Assert: いつものテンポで、色づかせない
        assert_eq!(mood.tempo(), 1.0);
        assert_eq!(mood.pigment_stimulus(), 0.0);
        assert_eq!(mood.anticipation(), 0.0);
    }

    #[test]
    fn disappointment_slows_the_body_in_proportion() {
        // Arrange
        let mut mood = Mood::new();

        // Act / Assert: がっかりしきると×0.6、半分なら×0.8
        mood.pin(0.0, 1.0);
        assert!((mood.tempo() - 0.6).abs() < 1e-6, "got {}", mood.tempo());
        mood.pin(0.0, 0.5);
        assert!((mood.tempo() - 0.8).abs() < 1e-6, "got {}", mood.tempo());
    }

    #[test]
    fn waiting_for_care_is_what_flushes_the_pigment() {
        // Arrange
        let mut mood = Mood::new();

        // Act
        mood.pin(0.7, 0.0);

        // Assert
        assert_eq!(mood.pigment_stimulus(), 0.7);
    }

    #[test]
    fn a_pinned_mood_is_not_overwritten_by_the_clock() {
        // Arrange: 待っている状態に固定する
        let mut mood = Mood::new();
        let (anticipation, disappointment) = MoodState::Waiting.anticipation_and_disappointment();
        mood.pin(anticipation, disappointment);

        // Act: 何も学んでいない個体に時刻が来る(本来なら期待は 0 に戻る)
        let noon = 20_000 * 86_400 + 12 * 3_600;
        for minute in 0..10 {
            mood.tick_clock(noon + minute * 60);
        }

        // Assert
        assert_eq!(mood.anticipation(), 1.0);
        assert_eq!(mood.disappointment(), 0.0);
    }

    #[test]
    fn every_state_can_be_named_and_found_again() {
        // Arrange / Act / Assert
        for state in MoodState::ALL {
            assert_eq!(MoodState::from_name(state.name()), Some(state));
        }
        assert_eq!(MoodState::from_name("sleepy"), None);
    }

    #[test]
    fn a_visit_before_knowing_the_time_teaches_nothing() {
        // Arrange
        let mut mood = Mood::new();

        // Act: 時刻を知らないまま世話が来る
        mood.record_visit();

        // Assert: いつ来たか分からないので、何も覚えていない
        assert_eq!(mood.memory(), CareMemory::default());
    }

    #[test]
    fn a_visit_at_a_known_time_is_learned() {
        // Arrange
        let mut mood = Mood::new();
        let evening = 20_000 * 86_400 + 20 * 3_600;
        mood.tick_clock(evening);

        // Act: 世話が来て、次の1分へ進む
        mood.record_visit();
        mood.tick_clock(evening + 60);

        // Assert
        assert_ne!(mood.memory(), CareMemory::default());
    }
}
