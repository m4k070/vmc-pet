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
    Animal, AutonomousController, BodyPort, CellPos, ControllerParams, FieldView, LeniaBody,
    Perturbation, PetMemory, Touch, TouchEcho, TouchEchoView,
};

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
    /// 触れている(またはホバーしている)位置。撫でている扱いで、
    /// `tick_input` のたびに echo を光らせ続ける。
    touching_at: Option<CellPos>,
    /// 人間のタッチとは独立に、体の形を見て自分から軽くならす自律コントローラ。
    controller: AutonomousController,
}

impl Pet {
    /// 生物を場の中央に配置して作る。自律コントローラは既定のパラメータで動く。
    pub fn new(animal: Animal, width: usize, height: usize) -> Self {
        Self::with_controller_params(animal, width, height, ControllerParams::default())
    }

    /// `code`(assets/animals.json のコード)から生物を読み込んで作る便利関数。
    pub fn load(code: &str, width: usize, height: usize) -> Result<Self, crate::animal::AnimalError> {
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
        Self {
            body: LeniaBody::new(animal, width, height),
            echo: TouchEcho::new(width, height),
            touching_at: None,
            controller: AutonomousController::with_params(controller_params),
        }
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
        self.body.step();
        // 自律コントローラは人間のタッチとは独立に、体の形を見てそれ自体を
        // ならす。`disturb` を通すため、これによってエネルギーは変化しない
        // (`LeniaBody::disturb` のドキュメント参照)。
        if let Some(perturbation) = self.controller.maybe_act(self.body.observe()) {
            self.body.disturb(perturbation);
        }
        if self.body.mass() < COLLAPSE_MASS {
            self.body.revive();
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
        }
    }

    /// 保存されていた状態を復元し、起動していなかった時間ぶんの減衰を適用する。
    /// 起動時に一度だけ呼ぶ。`seconds_away` は呼び出し側(時計を持つ層)が求める。
    pub fn restore(&mut self, memory: PetMemory, seconds_away: f32) {
        self.body.restore_energy(memory.energy);
        self.body.apply_offline_decay(seconds_away);
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

    /// 触れ方を、体への摂動と echo への摂動にそれぞれ翻訳して渡す。
    /// 体に働きかける経路はここだけ。
    fn touch(&mut self, touch: Touch) {
        if let Some(perturbation) = body_perturbation_for(touch) {
            self.body.inject(perturbation);
        }
        if let Some(perturbation) = echo_perturbation_for(touch) {
            self.echo.touch(&perturbation);
        }
    }

    pub fn observe(&self) -> FieldView<'_> {
        self.body.observe()
    }

    pub fn echo_view(&self) -> TouchEchoView<'_> {
        self.echo.view()
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
        assert!(pet.mass() > 0.0, "mass should still be present after one step, got {}", pet.mass());
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
        assert_eq!(pet.mass(), mass_after_click, "hovering must not touch the body");
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
        assert_eq!(pet.mass(), mass_after_click, "leaving must not touch the body");
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
        // Arrange: 場じゅうに最大の摂動を撃ち込んで焼き払う
        let mut pet = orbium();
        for _ in 0..12 {
            for y in (0..32).step_by(4) {
                for x in (0..32).step_by(4) {
                    pet.click(CellPos { x, y });
                    pet.leave();
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
        assert_eq!(pet.energy(), 0.0, "energy must still bottom out despite the controller");
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
        assert!(resumed_energy > 0.0, "a night away must not fully drain it, got {resumed_energy}");
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

    #[test]
    fn a_corrupt_memory_cannot_push_energy_out_of_range() {
        // Arrange: 保存ファイルが壊れて、値域外の値が入っていた場合
        let mut pet = orbium();

        // Act
        pet.restore(PetMemory { energy: 99.0 }, 0.0);

        // Assert: 上限に丸められる(体の値域の不変条件は保存ファイルより強い)
        assert_eq!(pet.energy(), 1.0);
    }
}
