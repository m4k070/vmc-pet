//! 体(世界モデル)・入力の翻訳・安全装置(崩壊検知)・echo をまとめて束ねる。
//!
//! PC版(`app.rs`)とM5Stack版(`m5stack-cores3/src/bin/main.rs`)は、
//! 「いつ進めるか」(std::time vs esp_hal::time、追いつき方の方針)と
//! 「どう入力を受け取り、どう描くか」(Wayland vs FT6336U/embedded-graphics)
//! が異なる。それ以外――1ステップ進めるたびに崩壊していないか確認して
//! 置き直すか、触れ方をどう体とechoに翻訳するか――は完全に同じであるべきなのに、
//! 以前はそれぞれが自前で書いていた(M5Stack版には崩壊検知が無い、という
//! 見落としが実際にあった)。ここに一本化することで、両者が同じ安全装置・
//! 同じ入力の意味づけを共有する。
//!
//! 逆に言えば、ここは「いつ」(時計・カデンス・追いつき方の方針)にも
//! 「どう見えるか」(画素)にも触れない。それらはどちらも各プラットフォーム
//! 固有の事情(PC版はまとめて追いつこうとしてから諦める、M5Stack版は毎回
//! 1ステップだけ進めて自然に遅れる、など)を持つため、この境界の外に置いてある。

use crate::touch::{body_perturbation_for, echo_perturbation_for};
use crate::{
    Animal, AutonomousController, BodyPort, CellPos, ControllerParams, FieldView, Habituation,
    LeniaBody, Mood, Observation, Perturbation, PetMemory, Pigment, PigmentView, Touch, TouchEcho,
    TouchEchoView,
};

/// 自律コントローラが働きかけたことを echo として光らせる強さ。
///
/// 体への摂動そのものを強めて動きを見せる道は行き止まりだった —— 出荷する精度へ
/// 丸めるだけで崩壊するかどうかが反転してしまう(docs/DESIGN.md「丸めで崩壊が
/// 反転した」参照)。echo は体に一切影響しない描画専用データなので、安定性とは
/// 無関係に好きなだけ目立たせられる。「ホバーで光らせようとすると体が死ぬ」
/// 問題を echo で構造的に解いたのと同じ手を、もう一度使っている
/// (docs/DESIGN.md「入力の可視化を体の場から分離する」参照)。
///
/// クリックの echo(1.0 の強いフラッシュ)より弱くしてある。突かれたのではなく
/// 自分で身じろぎした、という見え方を狙った値。
const SELF_ACTION_ECHO_AMOUNT: f32 = 0.5;

/// 体が崩壊したとみなす総量。健全な Orbium はおよそ 73.7 を保つ。
/// Lenia はカオス系で、摂動の強さを絞っても履歴次第では崩壊しうるため、
/// 崩壊を検知して置き直す。これがないとペットが二度と戻らない。
const COLLAPSE_MASS: f32 = 5.0;

/// 体(世界モデル)・入力の翻訳・崩壊検知・echo を束ねたもの。
/// PC版・M5Stack版で共通して使う。
pub struct Pet {
    body: LeniaBody,
    /// 入力を可視化するためだけのデータ。体の場とは別に持ち、体には一切影響しない。
    echo: TouchEcho,
    /// 体に付く色素。世話を待っているときに色づく(体の動きには影響しない)。
    pigment: Pigment,
    /// 触れている(またはホバーしている)位置。撫でている扱いで、
    /// `tick_input` のたびに echo を光らせ続ける。
    touching_at: Option<CellPos>,
    /// 人間のタッチとは独立に、体の形を見て自分から軽くならす自律コントローラ。
    controller: AutonomousController,
    /// 同じ場所への刺激に慣れる度合い。触れられた場所ごとに覚え、次の刺激を弱める。
    habituation: Habituation,
    /// 気分(世話がいつ来るかの学習と、期待・がっかり)。体のテンポと色素への刺激を決める。
    mood: Mood,
}

impl Pet {
    /// 生物を場の中央に配置して作る。自律コントローラは既定のパラメータで動く。
    pub fn new(animal: Animal, width: usize, height: usize) -> Self {
        Self::with_controller_params(animal, width, height, ControllerParams::default())
    }

    /// `code`(assets/animals.json のコード)から生物を読み込んで作る便利関数。
    pub fn load(
        code: &str,
        width: usize,
        height: usize,
    ) -> Result<Self, crate::animal::AnimalError> {
        Ok(Self::new(crate::load_animal(code)?, width, height))
    }

    /// 自律コントローラのパラメータを指定して作る。パラメータ探索
    /// (`crates/vmc-pet-body/examples/search_controller_params.rs`)や、
    /// 既定値以外を試したい場面で使う。
    pub fn with_controller_params(
        animal: Animal,
        width: usize,
        height: usize,
        controller_params: ControllerParams,
    ) -> Self {
        let mut pet = Self {
            body: LeniaBody::new(animal, width, height),
            echo: TouchEcho::new(width, height),
            pigment: Pigment::new(width, height),
            touching_at: None,
            controller: AutonomousController::with_params(controller_params),
            habituation: Habituation::new(width, height),
            mood: Mood::new(),
        };
        // 最初の1ステップより前に触られても、光が体に貼りつくようにしておく。
        // これを忘れると、最初のステップで原点が(0, 0)から重心へ跳び、
        // それまでに触った跡が一緒に跳んで見える。
        let centroid = pet.body_centroid();
        pet.echo.follow_body(centroid);
        pet.pigment.follow_body(centroid);
        pet
    }

    /// 体を1ステップ進め、崩壊していたら置き直す。
    ///
    /// 「本来いつ進めるべきか」(時計・カデンス・追いつき方の方針)は呼び出し側の
    /// 責任とする。PC版とM5Stack版で、どこまで追いつこうとするか・遅れをどう
    /// 切り捨てるかの方針そのものが異なる(PC版はまとめて追いつこうとしてから
    /// 諦める、M5Stack版は毎回1ステップだけ進めて自然に遅れる)ため、ここでは
    /// 「1回呼ばれたら1ステップ進める」という最小の単位だけを持つ。崩壊検知・
    /// 置き直しは、この最小単位そのものに必ず伴うべき安全装置なのでここに含める
    /// (以前M5Stack版だけこれが漏れていた)。
    ///
    /// 戻り値は、このステップで崩壊を検知して置き直したかどうか。
    pub fn step(&mut self) -> bool {
        // がっかりしているほど体の時間がゆっくり進む(表情としてのテンポ)
        self.body.set_tempo(self.mood.tempo());
        self.body.step();
        // 光(echo)と色素は体に貼りつけて覚えるので、体が進んだらすぐ重心を渡す。
        // 下の自律行動の光も、進んだ後の体を基準に記録される。
        let centroid = self.body_centroid();
        self.echo.follow_body(centroid);
        self.pigment.follow_body(centroid);
        // 世話を待っているほど、体があるところに色素が溜まる(ゆっくり色づく)
        self.pigment
            .step(self.body.observe(), self.mood.pigment_stimulus());
        // 慣れは体の時間に乗せて薄れていく。描画フレームではなくここで進めるのは、
        // フレームレートが PC と M5Stack で違うのに対し、体のステップはどちらも
        // 15/s で揃っているため(habituation.rs 参照)。
        self.habituation.recover();
        // 自律コントローラは人間のタッチとは独立に、体の形を見てそれ自体を
        // ならす。`disturb` を通すため、これによってエネルギーは変化しない
        // (`LeniaBody::disturb` のドキュメント参照)。
        let observation = Observation {
            field: self.body.observe(),
            energy: self.body.energy(),
            anticipation: self.mood.anticipation(),
        };
        if let Some(perturbation) = self.controller.maybe_act(observation) {
            self.body.disturb(perturbation);
            // 体への効き目は安全な弱さに保ったまま、echo 側で見えるようにする。
            // 位置と広がりは実際の摂動と同じものを使い、強さだけ差し替える。
            self.echo.touch(&Perturbation {
                amount: SELF_ACTION_ECHO_AMOUNT,
                ..perturbation
            });
        }
        if self.body.mass() < COLLAPSE_MASS {
            self.body.revive();
            // 置き直すと重心が跳ぶので、光と色素の基準も合わせる
            let centroid = self.body_centroid();
            self.echo.follow_body(centroid);
            self.pigment.follow_body(centroid);
            true
        } else {
            false
        }
    }

    /// 環境ストレス(0.0..=1.0 目安)を体に伝える。`step` とは独立に、
    /// 呼び出し側が必要なときだけ呼ぶ(例: PC版のCPU負荷。M5Stack版は呼ばない)。
    pub fn apply_environmental_stress(&mut self, stress: f32) {
        self.body.apply_environmental_stress(stress);
    }

    /// いま持ち越すべき状態を取り出す(保存用)。
    pub fn memory(&self) -> PetMemory {
        PetMemory {
            energy: self.body.energy(),
            care: self.mood.memory(),
        }
    }

    /// 保存されていた状態を復元し、起動していなかった時間ぶんの減衰を適用する。
    /// 起動時に一度だけ呼ぶ。`seconds_away` は呼び出し側(時計を持つ層)が求める。
    ///
    /// 学んだ生活リズムも復元する。止まっていた時間は忘れる理由にはしない
    /// (見ていなかっただけで、リズムそのものが変わったわけではない)。
    pub fn restore(&mut self, memory: PetMemory, seconds_away: f32) {
        self.body.restore_energy(memory.energy);
        self.body.apply_offline_decay(seconds_away);
        self.mood.restore(memory.care);
    }

    /// 触れている(またはホバーしている)位置を更新するだけで、体には一切触れない。
    ///
    /// 「これが新しいクリックか」の判定はここでは持たない。何を新規のクリックと
    /// みなすかは入力方式ごとに大きく異なる(PC版はウィンドウシステムが送ってくる
    /// 明示的な Press イベント、M5Stack版はタッチコントローラの状態遷移から自前で
    /// 判定する必要がある。docs/M5STACK.md「タッチしても無反応な場合が多い問題」
    /// 参照)ため、判定は呼び出し側の責任とし、ここでは `click` を明示的に
    /// 呼んでもらう。
    pub fn hover_at(&mut self, at: Option<CellPos>) {
        self.touching_at = at;
    }

    /// 指定位置への明示的なクリックを体と echo に届ける。
    /// 「これがクリックである」という判定は呼び出し側の責任。
    pub fn click(&mut self, at: CellPos) {
        self.touching_at = Some(at);
        // 世話が来た時刻として覚える。慣れた場所へのクリックでも、ユーザーが
        // 来たことには変わりないので数える(予測するのは「いつ来るか」であって、
        // 世話の質ではない)。
        self.mood.record_visit();
        self.touch(Touch::Click { at });
    }

    /// 明示的に離れたことを届ける。体・echo への影響は無い
    /// (`Touch::Leave` はどちらの摂動関数からも `None` を返す)が、
    /// 意味を残すために呼ぶ。
    pub fn leave(&mut self) {
        self.touching_at = None;
        self.touch(Touch::Leave);
    }

    /// 毎フレーム/毎ポーリングで呼ぶ。触れ続けている間は echo を光らせ続け、
    /// echo 全体を減衰させる。体の時間(`advance`)とは独立して呼んでよい。
    pub fn tick_input(&mut self, echo_decay: f32) {
        if let Some(at) = self.touching_at {
            self.touch(Touch::Hover { at });
        }
        self.echo.decay(echo_decay);
    }

    /// いまの時刻(Unix 秒)を知らせる。描画や体のステップのたびに呼んでよい。
    ///
    /// `Pet` は時計を読まない(既存の方針)ので、時刻は呼び出し側が渡す。これを
    /// 呼ぶと世話がいつ来るかの学習が進み、期待が振る舞いに現れる。呼ばなければ
    /// 何も学ばず、期待も 0.0 のまま(docs/DESIGN.md「世話の予測」)。
    pub fn tick_clock(&mut self, now_unix_seconds: u64) {
        self.mood.tick_clock(now_unix_seconds);
    }

    /// いま世話が来ることをどれだけ期待しているか(0.0..=1.0)。
    pub fn anticipation(&self) -> f32 {
        self.mood.anticipation()
    }

    /// 期待していたのに世話が来なかったことの溜まり具合(0.0..=1.0)。
    pub fn disappointment(&self) -> f32 {
        self.mood.disappointment()
    }

    /// 期待とがっかりを固定する。固定した後は `tick_clock` が来ても値が変わらない
    /// (学習そのものは続く)。
    ///
    /// 用途は2つある。
    ///
    /// - **評価**(`fitness::mood_trajectory`): 「元気/待っている/がっかり」を見た目で
    ///   見分けられるかを測るとき、何日ぶんも学習させて条件を作るのは遅いうえ、
    ///   クリックが場を乱してコントローラの貢献と区別がつかなくなる(エネルギーを
    ///   固定して条件を作るのと同じ理由)
    /// - **プレビュー**(PC の `--preview-mood`、M5Stack の `VMC_PET_PREVIEW_MOOD`):
    ///   生活リズムを覚えるのを何日も待たずに、その気分の見た目を確かめる
    pub fn pin_mood(&mut self, anticipation: f32, disappointment: f32) {
        self.mood.pin(anticipation, disappointment);
    }

    /// 触れ方を、体への摂動と echo への摂動にそれぞれ翻訳して渡す。
    /// 体に働きかける経路はここだけ。
    fn touch(&mut self, touch: Touch) {
        if let Some(perturbation) = body_perturbation_for(touch) {
            // 同じ場所を続けて触られるほど効かなくなる(慣れ)。効き目を先に
            // 読んでから慣れを進めるので、真新しい場所への最初の一撃は必ず
            // 満額で効く。慣れの広がる範囲は摂動そのものと同じ `at`/`radius`
            // を使う(habituation.rs 参照)。
            // 慣れは体の部位ごとに覚えるので、そのときの体の中心を添える。
            let body_centre = self.body_centre();
            let attention = self.habituation.attention_at(perturbation.at, body_centre);
            self.habituation.record(&perturbation, body_centre);
            // 場への効き目と、世話として数える度合い(エネルギーの回復)の
            // 両方を同じ `attention` で弱める。同じ場所を機械的に叩き続けるのは
            // 世話ではない、という扱い。当初は「弱った体を叩いても回復しないと
            // 壊れて見える」のを恐れて場だけを弱めていたが、慣れは場所ごとで
            // 数十秒で抜けるので、別の場所を触れば必ず満額で回復する
            // (docs/DESIGN.md「慣れ(同じ場所への刺激が効かなくなる)」参照)。
            self.body.disturb(Perturbation {
                amount: perturbation.amount * attention,
                ..perturbation
            });
            self.body.receive_care(attention);
        }
        if let Some(perturbation) = echo_perturbation_for(touch) {
            self.echo.touch(&perturbation);
        }
    }

    /// その場所の刺激がいまどれだけ効くか(1.0 = そのまま効く、0.0 = 慣れきって
    /// 効かない)。描画側が「慣れ」を可視化したくなったときのための窓口。
    pub fn attention_at(&self, at: CellPos) -> f32 {
        self.habituation.attention_at(at, self.body_centre())
    }

    /// 体の中心(重心)に最も近いセル。慣れを体の部位ごとに覚えるための基準。
    ///
    /// 場が空(重心が無い)のときは原点を返す。そのときは慣れが世界の座標で
    /// 記録されるだけで、体が無い以上それで困ることはない。
    fn body_centre(&self) -> CellPos {
        let field = self.body.observe();
        let (x, y) = self.body_centroid();
        // 重心は 0..width の範囲にあるので、0.5 を足して切り捨てれば四捨五入になる
        // (no_std では f32::round が使えない)。
        CellPos {
            x: (x + 0.5) as usize % field.width(),
            y: (y + 0.5) as usize % field.height(),
        }
    }

    /// 体の重心(場の座標、小数)。光(echo)を体に貼りつける基準。
    ///
    /// 慣れはセル単位で足りるので `body_centre` で丸めて使うが、光は描画の
    /// たびに読まれるので、丸めずに渡して補間で読む(touch_echo.rs 参照)。
    /// 場が空のときは原点を返す。
    fn body_centroid(&self) -> (f32, f32) {
        self.body
            .observe()
            .toroidal_centroid()
            .unwrap_or((0.0, 0.0))
    }

    pub fn observe(&self) -> FieldView<'_> {
        self.body.observe()
    }

    pub fn echo_view(&self) -> TouchEchoView<'_> {
        self.echo.view()
    }

    /// 体に付く色素(世話を待っているときの色づき)。描画側が体の色に混ぜる。
    pub fn pigment_view(&self) -> PigmentView<'_> {
        self.pigment.view()
    }

    pub fn mass(&self) -> f32 {
        self.body.mass()
    }

    pub fn energy(&self) -> f32 {
        self.body.energy()
    }

    pub fn touching_at(&self) -> Option<CellPos> {
        self.touching_at
    }
}

/// 通常の `click`/`hover_at` が通す安全域を経由せず、体へ直接摂動を注入したい
/// 場面(体そのものの頑健性テストなど)のための、素の窓口。
impl BodyPort for Pet {
    fn inject(&mut self, perturbation: Perturbation) {
        self.body.inject(perturbation);
    }

    fn observe(&self) -> FieldView<'_> {
        self.body.observe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn orbium() -> Pet {
        Pet::load("O2u", 32, 32).unwrap()
    }

    #[test]
    fn step_advances_the_body_by_one_step() {
        // Arrange
        let mut pet = orbium();

        // Act
        let collapsed = pet.step();

        // Assert: 健常な体は1ステップで崩壊しない
        assert!(!collapsed);
        assert!(
            pet.mass() > 0.0,
            "mass should still be present after one step, got {}",
            pet.mass()
        );
    }

    #[test]
    fn a_click_inside_the_field_feeds_the_body() {
        // Arrange
        let mut pet = orbium();
        let before = pet.mass();

        // Act: 生物のいない隅を押す
        pet.click(CellPos { x: 2, y: 2 });

        // Assert
        assert!(pet.mass() > before, "a click must inject energy");
    }

    #[test]
    fn a_click_also_lights_up_the_echo() {
        // Arrange
        let mut pet = orbium();

        // Act
        pet.click(CellPos { x: 2, y: 2 });

        // Assert
        assert!(pet.echo_view().get(2, 2) > 0.0);
    }

    #[test]
    fn hovering_does_not_click() {
        // Arrange
        let mut pet = orbium();
        let before = pet.mass();

        // Act: ホバーだけを繰り返す(PC版のマウス移動、M5Stack版の
        // 「押した瞬間」と判定していない状態遷移に相当)
        pet.hover_at(Some(CellPos { x: 2, y: 2 }));
        pet.hover_at(Some(CellPos { x: 2, y: 2 }));

        // Assert: hover_at 自身は決してクリックにならない
        assert_eq!(pet.mass(), before);
        assert_eq!(pet.touching_at(), Some(CellPos { x: 2, y: 2 }));
    }

    #[test]
    fn tick_input_keeps_lighting_the_echo_while_touching_without_touching_the_body() {
        // Arrange
        let mut pet = orbium();
        pet.click(CellPos { x: 2, y: 2 });
        let mass_after_click = pet.mass();

        // Act: 触れたまま何度も tick する(PC版の毎フレーム描画、M5Stack版の
        // 毎ポーリングに相当)
        for _ in 0..10 {
            pet.tick_input(0.9);
        }

        // Assert: echo は光り続けるが、体の総量はクリックの瞬間から変わらない
        assert!(pet.echo_view().get(2, 2) > 0.0);
        assert_eq!(
            pet.mass(),
            mass_after_click,
            "hovering must not touch the body"
        );
    }

    #[test]
    fn leave_clears_the_touching_position_without_touching_the_body() {
        // Arrange
        let mut pet = orbium();
        pet.click(CellPos { x: 2, y: 2 });
        let mass_after_click = pet.mass();

        // Act
        pet.leave();

        // Assert
        assert_eq!(pet.touching_at(), None);
        assert_eq!(
            pet.mass(),
            mass_after_click,
            "leaving must not touch the body"
        );
    }

    #[test]
    fn clicking_again_after_leaving_injects_a_second_time() {
        // Arrange
        let mut pet = orbium();
        pet.click(CellPos { x: 2, y: 2 });
        let mass_after_first_click = pet.mass();

        // Act: 離してから同じ場所にもう一度クリックする
        pet.leave();
        pet.click(CellPos { x: 2, y: 2 });

        // Assert: 2回目のクリックでさらに注入される
        assert!(pet.mass() > mass_after_first_click);
    }

    #[test]
    fn a_collapsed_body_is_revived_by_step() {
        // Arrange: 場じゅうに最大の摂動を撃ち込んで焼き払う。
        //
        // クリックではなく素の注入窓口(`BodyPort`)を使う。クリックは慣れ
        // (habituation)で弱まるようになったため、同じ場所を繰り返し叩いても
        // もう体を壊せない。ここで確かめたいのは安全装置(崩壊検知と置き直し)
        // そのものなので、入力の意味づけを経由しない経路で壊す。
        let mut pet = orbium();
        for _ in 0..12 {
            for y in (0..32).step_by(4) {
                for x in (0..32).step_by(4) {
                    pet.inject(Perturbation {
                        at: CellPos { x, y },
                        radius: 6.0,
                        amount: 1.0,
                    });
                }
            }
            pet.step();
        }

        // Act: 崩壊を検知させる
        let mut revived = false;
        for _ in 0..900 {
            if pet.step() {
                revived = true;
            }
        }

        // Assert
        assert!(revived, "a collapsing body must be revived by step");
        assert!(pet.mass() > 40.0);
    }

    /// 自律コントローラ(体の形を見て自分から軽くならす)が常に動いている状態で、
    /// 長時間(20000ステップ ≈ 22分ぶん)動かしても崩壊しないことを確かめる。
    /// `AutonomousController` を実際に組み込んだのは `Pet::step` なので、ここで
    /// 検証する。
    #[test]
    fn the_autonomous_controller_never_collapses_the_body_over_a_long_run() {
        // Arrange
        let mut pet = orbium();

        // Act
        let mut collapsed_at_least_once = false;
        for _ in 0..20_000 {
            if pet.step() {
                collapsed_at_least_once = true;
            }
        }

        // Assert: 自律コントローラの自己摂動だけで崩壊しないこと
        assert!(
            !collapsed_at_least_once,
            "the autonomous controller must not collapse a healthy, untouched body"
        );
        assert!(pet.mass() > 40.0, "got {}", pet.mass());
    }

    /// 場じゅうで一番明るい echo の値。
    fn brightest_echo(pet: &Pet) -> f32 {
        let echo = pet.echo_view();
        let mut brightest = 0.0f32;
        for y in 0..echo.height() {
            for x in 0..echo.width() {
                brightest = brightest.max(echo.get(x, y));
            }
        }
        brightest
    }

    #[test]
    fn the_pets_own_action_lights_the_echo_without_being_touched() {
        // Arrange: 一切触れない
        let mut pet = orbium();
        assert_eq!(
            brightest_echo(&pet),
            0.0,
            "nothing should glow before anything happens"
        );

        // Act: コントローラが判断する間隔(既定17ステップ)を超えて進める
        let mut lit = false;
        for _ in 0..40 {
            pet.step();
            if brightest_echo(&pet) > 0.0 {
                lit = true;
                break;
            }
        }

        // Assert: 触られていないのに光る(自分から動いたことが見える)
        assert!(
            lit,
            "the controller's own action must be visible through the echo"
        );
    }

    #[test]
    fn the_pets_own_action_does_not_count_as_being_cared_for() {
        // Arrange: echo で光るようになっても、それは世話ではない
        let mut pet = orbium();
        for _ in 0..10_000 {
            pet.step();
        }

        // Act / Assert: 自分で身じろぎし続けてもエネルギーは尽きる
        assert_eq!(pet.energy(), 0.0);
    }

    /// 自律コントローラは**全生物に出荷される**。にもかかわらず、これまでの
    /// 安全性検証は O2u(Orbium)だけで行っていた。OG2g が刺激の繰り返しに
    /// 脆いことが分かった以上(docs/DESIGN.md「未確定(実装しながら決める)」参照)、コントローラの
    /// 周期的な自己摂動が別の生物を壊す可能性がある。クリックと違って
    /// コントローラは勝手に発火するので、これが起きるとユーザーが何もして
    /// いないのに体が壊れる。
    ///
    /// パラメータを変えるたび(とくに自動探索で選んだ値を採用するとき)に
    /// この不変条件が崩れていないか確かめるためのテスト。
    #[test]
    fn the_controller_keeps_every_shipped_animal_alive() {
        for (code, name) in crate::list_animals().unwrap() {
            // Arrange
            let mut pet = Pet::load(&code, 32, 32).unwrap();

            // Act: 20000ステップ(≈22分)、一切触らずに動かす
            let mut collapses = 0u32;
            for _ in 0..20_000 {
                if pet.step() {
                    collapses += 1;
                }
            }

            // Assert
            assert_eq!(
                collapses, 0,
                "the controller collapsed {code} ({name}) without any user input"
            );
            assert!(
                pet.mass() > 40.0,
                "{code} ({name}) must stay alive, got mass {}",
                pet.mass()
            );
        }
    }

    /// 世話が来そうな時間は、弱っていても満タン時の強さで揺らす(先回り)。その
    /// 最悪のケース —— 一日じゅう期待し続け、世話は一切来ない —— でも全生物が
    /// 生き延びることを確かめる。
    ///
    /// 期待が常に 1.0 のときの揺らす強さは、`nudge_amount_when_depleted` を 1.0 に
    /// したときと式の上で完全に一致する(controller.rs の vigour)ので、何日ぶんも
    /// 学習させる代わりにそれで再現している。
    #[test]
    fn every_shipped_animal_survives_always_expecting_care_that_never_comes() {
        let always_expecting = ControllerParams {
            nudge_amount_when_depleted: 1.0,
            ..ControllerParams::default()
        };
        for (code, name) in crate::list_animals().unwrap() {
            // Arrange
            let animal = crate::load_animal(&code).unwrap();
            let mut pet = Pet::with_controller_params(animal, 32, 32, always_expecting);

            // Act: 一切触らずに20000ステップ(≈22分)。エネルギーは早々に尽きる
            let mut collapses = 0u32;
            for _ in 0..20_000 {
                if pet.step() {
                    collapses += 1;
                }
            }

            // Assert
            assert_eq!(
                collapses, 0,
                "stirring a depleted {code} ({name}) at full strength collapsed it"
            );
            assert!(
                pet.mass() > 40.0,
                "{code} ({name}) must stay alive, got mass {}",
                pet.mass()
            );
        }
    }

    #[test]
    fn a_pet_without_a_clock_never_expects_care() {
        // Arrange
        let mut pet = orbium();
        let at = CellPos { x: 3, y: 3 };

        // Act: 時刻を知らせずに触り、体を進める
        for _ in 0..50 {
            pet.click(at);
            pet.leave();
            pet.step();
        }

        // Assert: 学びようがないので期待しない(評価・テストの振る舞いが変わらない)
        assert_eq!(pet.anticipation(), 0.0);
    }

    #[test]
    fn visits_at_the_same_hour_teach_the_pet_to_expect_care_then() {
        // Arrange: 1週間、毎晩20時台に2分おきにクリックしに来る。学ぶのは時刻と
        // クリックの関係だけで体の状態には依らないので、体を進めるのは省く
        const DAY: u64 = 86_400;
        let midnight = 20_000 * DAY;
        let mut pet = orbium();
        let at = CellPos { x: 3, y: 3 };

        // Act
        for day in 0..7 {
            for minute in 0..1_440 {
                pet.tick_clock(midnight + day * DAY + minute * 60);
                if minute / 60 == 20 && minute % 2 == 0 {
                    pet.click(at);
                    pet.leave();
                }
            }
        }
        pet.tick_clock(midnight + 7 * DAY + 8 * 3_600 + 30 * 60);
        let morning = pet.anticipation();
        pet.tick_clock(midnight + 7 * DAY + 20 * 3_600 + 30 * 60);
        let evening = pet.anticipation();

        // Assert: いつもの時間には世話を期待し、そうでない時間は期待しない
        assert!(evening > 0.5, "got evening anticipation {evening}");
        assert_eq!(morning, 0.0);

        // Act: 保存して、別の個体として再起動する(8時間止まっていたとする)
        let memory = pet.memory();
        let mut reborn = orbium();
        reborn.restore(memory, 8.0 * 3_600.0);
        reborn.tick_clock(midnight + 8 * DAY + 20 * 3_600 + 30 * 60);

        // Assert: 再起動しても、いつもの時間を覚えている
        let remembered = reborn.anticipation();
        assert!(
            remembered > 0.5,
            "the learned rhythm must survive a restart; got {remembered}"
        );
    }

    /// 体全体の平均の色づき具合(体の値で重みづけ)。
    fn mean_tint(pet: &Pet) -> f32 {
        let field = pet.observe();
        let pigment = pet.pigment_view();
        let (mut tinted, mut total) = (0.0f32, 0.0f32);
        for y in 0..field.height() {
            for x in 0..field.width() {
                let body_value = field.get(x, y);
                tinted += body_value * pigment.get(x, y);
                total += body_value;
            }
        }
        if total > 0.0 {
            tinted / total
        } else {
            0.0
        }
    }

    #[test]
    fn a_pet_waiting_for_care_flushes_and_a_lively_one_does_not() {
        // Arrange
        let mut waiting = orbium();
        waiting.pin_mood(1.0, 0.0);
        let mut lively = orbium();

        // Act: 1分ぶん進める
        for _ in 0..900 {
            waiting.step();
            lively.step();
        }

        // Assert: 待っている個体ははっきり色づき、元気な個体は色づかない
        let waiting_tint = mean_tint(&waiting);
        assert!(
            waiting_tint > 0.3,
            "a waiting pet must flush; got {waiting_tint}"
        );
        assert_eq!(mean_tint(&lively), 0.0);
    }

    #[test]
    fn the_pigment_never_changes_how_the_body_moves() {
        // Arrange: 期待で揺らす強さが上がるぶんは、同じ強さを直接設定した個体と揃える。
        // 違いは色素が溜まっているかどうかだけになる
        let mut flushed = orbium();
        flushed.pin_mood(1.0, 0.0);
        let same_stirring = ControllerParams {
            nudge_amount_when_depleted: 1.0,
            ..ControllerParams::default()
        };
        let mut plain =
            Pet::with_controller_params(crate::load_animal("O2u").unwrap(), 32, 32, same_stirring);

        // Act
        for _ in 0..3_000 {
            flushed.step();
            plain.step();
        }

        // Assert: 体の場は1ビットも違わない(色素は体 → 色素の一方向だけ)
        assert!(
            mean_tint(&flushed) > 0.3,
            "the pigment must actually be there"
        );
        assert_eq!(flushed.mass(), plain.mass());
        assert_eq!(
            flushed.observe().toroidal_centroid(),
            plain.observe().toroidal_centroid()
        );
    }

    /// がっかりしきった体のテンポ(×0.6)で、世話が一切来ないまま長く過ごしても、
    /// 全生物が生き延びることを確かめる。がっかりは実際には数十分かけて溜まるので、
    /// エネルギーが自然に尽きてから750ステップかけて徐々にがっかりさせる
    /// (体側の軸を振り分けた計測と同じ条件の作り方)。
    #[test]
    fn every_shipped_animal_survives_a_long_disappointment() {
        for (code, name) in crate::list_animals().unwrap() {
            // Arrange
            let mut pet = Pet::load(&code, 32, 32).unwrap();
            let mut collapses = 0u32;
            for step in 0..3_000u32 {
                let disappointment = (step.saturating_sub(2_250) as f32 / 750.0).min(1.0);
                pet.pin_mood(0.0, disappointment);
                if pet.step() {
                    collapses += 1;
                }
            }

            // Act: がっかりしきったまま20000ステップ(≈22分)
            for _ in 0..20_000 {
                if pet.step() {
                    collapses += 1;
                }
            }

            // Assert
            assert_eq!(
                collapses, 0,
                "a long disappointment collapsed {code} ({name})"
            );
            assert!(
                pet.mass() > 40.0,
                "{code} ({name}) must stay alive, got mass {}",
                pet.mass()
            );
        }
    }

    #[test]
    fn missing_the_usual_visit_leaves_the_pet_disappointed() {
        // Arrange: 1週間、毎晩20時台に2分おきにクリックしに来る
        const DAY: u64 = 86_400;
        let midnight = 20_000 * DAY;
        let mut pet = orbium();
        let at = CellPos { x: 3, y: 3 };
        for day in 0..7 {
            for minute in 0..1_440 {
                pet.tick_clock(midnight + day * DAY + minute * 60);
                if minute / 60 == 20 && minute % 2 == 0 {
                    pet.click(at);
                    pet.leave();
                }
            }
        }
        assert_eq!(pet.disappointment(), 0.0, "every evening was visited");

        // Act: 8日目は夜を過ぎても来ない
        for minute in 0..(21 * 60 + 30) {
            pet.tick_clock(midnight + 7 * DAY + minute * 60);
        }

        // Assert
        let disappointment = pet.disappointment();
        assert!(disappointment > 0.8, "got {disappointment}");
    }

    /// 自律コントローラの自己摂動は `disturb` 経由でエネルギーを変えないため、
    /// 「放置されると弱る」という前提(docs/DESIGN.md参照)は、コントローラが
    /// 動いていても壊れないはずである。
    #[test]
    fn the_autonomous_controller_does_not_prevent_neglect_from_weakening_the_body() {
        // Arrange
        let mut pet = orbium();
        let healthy_mass = pet.mass();

        // Act: 一切タッチせずに20000ステップ進める(コントローラは動き続ける)
        for _ in 0..20_000 {
            pet.step();
        }

        // Assert: コントローラが「世話」の代わりになって放置の効果を消してはいない
        let neglected_mass = pet.mass();
        assert!(
            neglected_mass < healthy_mass * 0.98,
            "the controller must not substitute for real touch; \
             healthy={healthy_mass} neglected={neglected_mass}"
        );
        assert_eq!(
            pet.energy(),
            0.0,
            "energy must still bottom out despite the controller"
        );
    }

    #[test]
    fn memory_round_trips_when_no_time_has_passed() {
        // Arrange: 少し放置してエネルギーを減らした状態を保存する
        let mut pet = orbium();
        for _ in 0..500 {
            pet.step();
        }
        let saved = pet.memory();
        assert!(saved.energy < 1.0, "energy must have decayed before saving");

        // Act: 別個体として作り直し、間を置かずに復元する
        let mut resumed = orbium();
        resumed.restore(saved, 0.0);

        // Assert: 保存した気分状態がそのまま戻る
        assert_eq!(resumed.energy(), saved.energy);
    }

    #[test]
    fn a_night_away_weakens_the_pet_without_fully_draining_it() {
        // Arrange: 満タンで保存された状態
        let mut pet = orbium();
        let saved = pet.memory();
        assert_eq!(saved.energy, 1.0);

        // Act: 8時間(一晩ほど)離れてから復元する
        pet.restore(saved, 8.0 * 60.0 * 60.0);

        // Assert: はっきり弱っているが、尽き切ってはいない
        // (ユーザーと相談して選んだ「進むが、減衰はゆるやかに」の狙い)
        let resumed_energy = pet.energy();
        assert!(
            resumed_energy > 0.0,
            "a night away must not fully drain it, got {resumed_energy}"
        );
        assert!(
            resumed_energy < 0.5,
            "a night away must clearly weaken it, got {resumed_energy}"
        );
    }

    #[test]
    fn a_long_absence_drains_the_pet_but_never_goes_negative() {
        // Arrange
        let mut pet = orbium();
        let saved = pet.memory();

        // Act: 1週間離れる
        pet.restore(saved, 7.0 * 24.0 * 60.0 * 60.0);

        // Assert: 下限は守られる
        assert_eq!(pet.energy(), 0.0);
    }

    /// 場の総量が、クリック1回でどれだけ増えるかを測る。
    fn mass_gain_from_clicking(pet: &mut Pet, at: CellPos) -> f32 {
        let before = pet.mass();
        pet.click(at);
        pet.leave();
        pet.mass() - before
    }

    #[test]
    fn clicking_the_same_place_over_and_over_stops_affecting_the_body() {
        // Arrange: 生物から離れた隅(体の脈動に紛れない場所)を使う
        let mut pet = orbium();
        let at = CellPos { x: 3, y: 3 };
        let first_gain = mass_gain_from_clicking(&mut pet, at);
        assert!(
            first_gain > 0.0,
            "the first click must register, got {first_gain}"
        );

        // Act: 同じ場所を続けて叩く
        for _ in 0..4 {
            mass_gain_from_clicking(&mut pet, at);
        }
        let habituated_gain = mass_gain_from_clicking(&mut pet, at);

        // Assert: ほとんど効かなくなる
        assert!(
            habituated_gain < first_gain * 0.1,
            "clicking the same place must stop registering; \
             first={first_gain} habituated={habituated_gain}"
        );
    }

    #[test]
    fn a_different_place_still_registers_after_habituating_elsewhere() {
        // Arrange: 片方の隅に慣れさせる
        let mut pet = orbium();
        let familiar = CellPos { x: 3, y: 3 };
        let first_gain = mass_gain_from_clicking(&mut pet, familiar);
        for _ in 0..5 {
            mass_gain_from_clicking(&mut pet, familiar);
        }

        // Act: 反対側の隅を叩く
        let fresh_gain = mass_gain_from_clicking(&mut pet, CellPos { x: 28, y: 28 });

        // Assert: 慣れは場所ごとなので、新しい場所は満額で効く
        assert!(
            fresh_gain > first_gain * 0.8,
            "habituation must be specific to the place; first={first_gain} fresh={fresh_gain}"
        );
    }

    /// 体の中心から見て `(dx, dy)` だけ離れた、場のセル。
    ///
    /// 慣れは体の部位ごとに覚えるので、体が動いたあとに「同じ場所」を
    /// 触るには、世界の座標ではなくこれで位置を求める必要がある。
    fn on_body(pet: &Pet, dx: usize, dy: usize) -> CellPos {
        let centre = pet.body_centre();
        let field = pet.observe();
        CellPos {
            x: (centre.x + dx) % field.width(),
            y: (centre.y + dy) % field.height(),
        }
    }

    #[test]
    fn habituation_wears_off_so_the_same_place_registers_again() {
        // Arrange: 慣れきるまで叩く
        let mut pet = orbium();
        let at = CellPos { x: 3, y: 3 };
        let first_gain = mass_gain_from_clicking(&mut pet, at);
        for _ in 0..5 {
            mass_gain_from_clicking(&mut pet, at);
        }
        assert!(mass_gain_from_clicking(&mut pet, at) < first_gain * 0.1);
        let centre = pet.body_centre();
        let field_width = pet.observe().width();
        let field_height = pet.observe().height();
        let (dx, dy) = (
            (at.x + field_width - centre.x) % field_width,
            (at.y + field_height - centre.y) % field_height,
        );

        // Act: 触らずに45秒ぶん(15 step/s で 675 ステップ)進める
        for _ in 0..675 {
            pet.step();
        }

        // Assert: 体の同じ部位が、また慣れていない状態に戻っている。
        // 体はこの間に大きく移動しているので、世界の同じ座標(3, 3)を
        // 見ても回復を確かめたことにならない
        let attention = pet.attention_at(on_body(&pet, dx, dy));
        assert!(
            attention > 0.9,
            "habituation must wear off; attention on the same part of the body = {attention}"
        );
    }

    #[test]
    fn touching_the_same_spot_on_a_gliding_body_once_a_second_habituates() {
        // Arrange: O2u は1秒に約9セル滑るように進む。画面上(=体基準)で
        // 同じ位置を1秒おきに触る、M5Stack で実際に起きていた状況を再現する
        let mut pet = orbium();
        let (dx, dy) = (0, 10);

        // Act
        for _ in 0..6 {
            pet.click(on_body(&pet, dx, dy));
            pet.leave();
            for _ in 0..15 {
                pet.step();
            }
        }

        // Assert: 世界の座標では毎回違う場所だが、体の同じ部位として慣れる。
        // 場の座標で慣れを覚えていた頃は、同じ状況を模した計測で7回目でも
        // 9割以上効いていた(habituation.rs 先頭のコメント参照)
        let attention = pet.attention_at(on_body(&pet, dx, dy));
        assert!(
            attention < 0.3,
            "touching the same part of a moving body must habituate; attention = {attention}"
        );
    }

    #[test]
    fn the_glow_of_a_click_stays_on_the_body_while_it_glides() {
        // Arrange: 体の中心から見て下へ10セルの位置をクリックする
        let mut pet = orbium();
        let (dx, dy) = (0, 10);
        let clicked = on_body(&pet, dx, dy);
        pet.click(clicked);
        pet.leave();

        // Act: 1秒ぶん進める(O2u は約9セル滑る)。光の減衰は描画側
        // (tick_input)が進めるので、ここでは減らない
        for _ in 0..15 {
            pet.step();
        }

        // Assert: 光は体の同じ部位(=画面上で触った位置)に留まり、
        // 世界の元の座標には残らない
        let same_part = on_body(&pet, dx, dy);
        let moved = (same_part.x as i32 - clicked.x as i32).abs();
        assert!(
            moved >= 3,
            "the body must have glided for this test to mean anything; moved {moved}"
        );
        let echo = pet.echo_view();
        let on_the_body = echo.get(same_part.x, same_part.y);
        let left_behind = echo.get(clicked.x, clicked.y);
        assert!(
            on_the_body > 0.8,
            "the glow must stay on the body; got {on_the_body}"
        );
        assert!(
            on_the_body > left_behind,
            "the glow must not be left behind; on_body={on_the_body} left_behind={left_behind}"
        );
    }

    #[test]
    fn a_habituated_touch_does_not_count_as_care() {
        // Arrange: エネルギーを使い切ってから、同じ場所に慣れさせる
        let mut pet = orbium();
        for _ in 0..10_000 {
            pet.step();
        }
        let at = CellPos { x: 3, y: 3 };
        for _ in 0..6 {
            pet.click(at);
            pet.leave();
        }
        assert!(
            pet.attention_at(at) < 0.05,
            "must be fully habituated by now"
        );

        // Act: 慣れきった場所をさらに叩く
        let energy_before = pet.energy();
        pet.click(at);
        pet.leave();

        // Assert: 場に効かないのと同じく、機嫌もほとんど直らない
        let gain = pet.energy() - energy_before;
        assert!(
            gain < crate::lenia_body::ENERGY_PER_TOUCH * 0.05,
            "a habituated touch must not count as care; gained {gain}"
        );
    }

    #[test]
    fn a_fresh_place_still_counts_as_full_care_while_another_is_habituated() {
        // Arrange: 使い切ってから、ある場所にだけ慣れさせる
        let mut pet = orbium();
        for _ in 0..10_000 {
            pet.step();
        }
        let worn = CellPos { x: 3, y: 3 };
        for _ in 0..6 {
            pet.click(worn);
            pet.leave();
        }

        // Act: 離れた真新しい場所を触る
        let fresh = CellPos { x: 20, y: 20 };
        assert_eq!(
            pet.attention_at(fresh),
            1.0,
            "the fresh place must be untouched"
        );
        let energy_before = pet.energy();
        pet.click(fresh);
        pet.leave();

        // Assert: 満額で回復する。慣れは場所ごとなので、弱った体が
        // 「何をしても回復しない」状態にはならない
        let gain = pet.energy() - energy_before;
        assert!(
            (gain - crate::lenia_body::ENERGY_PER_TOUCH).abs() < 1e-6,
            "a fresh place must count as full care; gained {gain}"
        );
    }

    #[test]
    fn a_corrupt_memory_cannot_push_energy_out_of_range() {
        // Arrange: 保存ファイルが壊れて、値域外の値が入っていた場合
        let mut pet = orbium();

        // Act
        pet.restore(PetMemory::with_energy(99.0), 0.0);

        // Assert: 上限に丸められる(体の値域の不変条件は保存ファイルより強い)
        assert_eq!(pet.energy(), 1.0);
    }
}
