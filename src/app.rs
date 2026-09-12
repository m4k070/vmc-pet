//! ペット本体。体・IF層・描画・時間をここで束ねる。
//! 唯一の可変状態の持ち主であり、体には `BodyPort` 越しにしか働きかけない。

use std::time::{Duration, Instant};

use vmc_pet_body::{load_animal, BodyPort, LeniaBody};
use crate::interface::{body_perturbation_for, echo_perturbation_for, MachineLoad, Touch};
use crate::render::{Camera, DotGrid, TouchEcho};
use crate::shell::{InputRegion, PointerInput, Surface};

/// 場の解像度。表示と 1:1 にしてある。Orbium は 20x20 なので画面の 6 割強を占める。
/// この大きさでも生物の挙動が変わらないことは実測で確かめた(docs/DESIGN.md 参照)。
const FIELD_WIDTH: usize = 32;
const FIELD_HEIGHT: usize = 32;

/// 表示解像度。場を平均プーリングで落として描く。
const GRID_COLUMNS: usize = 32;
const GRID_ROWS: usize = 32;

/// 起動時に読み込む生物のデフォルト。assets/animals.json のコードを指す。
/// `--animal` 引数(main.rs)で上書きできる。
pub const DEFAULT_ANIMAL_CODE: &str = "O2u";

/// 場を進める頻度。描画レートとは独立に決める。
const STEPS_PER_SECOND: u32 = 15;

/// 体が崩壊したとみなす総量。健全な Orbium はおよそ 73.7 を保つ。
/// Lenia はカオス系で、摂動の強さを絞っても履歴次第では崩壊しうるため、
/// 崩壊を検知して置き直す。これがないとペットが二度と戻らない。
const COLLAPSE_MASS: f32 = 5.0;

/// 1回の描画でまとめて進めるステップ数の上限。
/// 復帰直後など大きく遅れた場合に、追いつこうとして固まるのを防ぐ。
const MAX_CATCH_UP_STEPS: u32 = 4;

/// echo(入力の可視化)の、毎フレームの減衰率。
/// `interface::pointer::HOVER_ECHO_AMOUNT_PER_FRAME` と組み合わさって定常値を決める
/// (docs/DESIGN.md「入力の可視化を体の場から分離する」参照)。この値では、
/// 撫で続けたときに約4秒で 0.8 前後へ収束し、離すと1〜2秒ほどで消える。
const ECHO_DECAY_PER_FRAME: f32 = 0.90;

/// Lenia の場を体として持つペット。
pub struct Pet {
    body: LeniaBody,
    /// 入力を可視化するためだけのデータ。体の場とは別に持ち、体には一切影響しない。
    echo: TouchEcho,
    grid: DotGrid,
    camera: Camera,
    step_interval: Duration,
    last_step: Instant,
    /// 直近のサーフェスの大きさ。ポインタ座標を場のセルへ写すのに要る。
    surface_size: (u32, u32),
    /// ポインタが体の上にある間の位置。撫でている扱いで、毎フレーム echo を光らせる。
    hovering_at: Option<vmc_pet_body::CellPos>,
    /// 機械の CPU 負荷を「環境の厳しさ」として体に伝えるための読み取り役。
    machine_load: MachineLoad,
}

impl Pet {
    /// `code` の生物を読み込んで体を作る。
    /// 生物データは実行ファイルに埋め込んであるため、読み込みに失敗するのは
    /// コードが存在しないか、データが壊れている場合だけで、その場合は起動を止める。
    pub fn new(code: &str) -> Result<Self, PetError> {
        let animal = load_animal(code).map_err(PetError::Animal)?;
        eprintln!(
            "vmc-pet: loaded {} ({}) R={} T={}",
            animal.name, animal.code, animal.params.radius, animal.params.time_divisor
        );

        Ok(Self {
            body: LeniaBody::new(animal, FIELD_WIDTH, FIELD_HEIGHT),
            echo: TouchEcho::new(FIELD_WIDTH, FIELD_HEIGHT),
            grid: DotGrid::new(GRID_COLUMNS, GRID_ROWS),
            camera: Camera::new(),
            step_interval: Duration::from_secs_f64(1.0 / STEPS_PER_SECOND as f64),
            last_step: Instant::now(),
            surface_size: (0, 0),
            hovering_at: None,
            machine_load: MachineLoad::new(),
        })
    }

    /// 前回のステップからの経過分だけ体を進める。
    /// 体に触れるのはクリックだけなので、ここではホバーを一切扱わない。
    fn advance(&mut self, now: Instant) {
        let mut steps = 0;
        while now.duration_since(self.last_step) >= self.step_interval && steps < MAX_CATCH_UP_STEPS
        {
            let energy_before = self.body.energy();
            self.body.step();
            // 機械が忙しいほど、環境が厳しくエネルギーが早く尽きるようにする
            self.body.apply_environmental_stress(self.machine_load.sample());
            if energy_before > 0.0 && self.body.energy() == 0.0 {
                eprintln!("vmc-pet: energy depleted; the body is weakening from neglect");
            }
            if self.body.mass() < COLLAPSE_MASS {
                eprintln!("vmc-pet: the body collapsed; reviving");
                self.body.revive();
            }
            self.last_step += self.step_interval;
            steps += 1;
        }
        if steps == MAX_CATCH_UP_STEPS {
            // 追いつけなかった分は捨てる
            self.last_step = now;
        }
    }

    /// 触れ方を、体への摂動と echo への摂動にそれぞれ翻訳して渡す。
    /// 体に働きかける経路はここだけ。ホバーは echo にしか届かない。
    fn touch(&mut self, touch: Touch) {
        if let Some(perturbation) = body_perturbation_for(touch) {
            self.body.inject(perturbation);
        }
        if let Some(perturbation) = echo_perturbation_for(touch) {
            self.echo.touch(&perturbation);
        }
    }

    /// ポインタ座標に対応する場のセル。グリッドの外なら `None`。
    fn cell_under(&self, x: f64, y: f64) -> Option<vmc_pet_body::CellPos> {
        self.grid.cell_at(
            (x, y),
            self.camera.origin(),
            (FIELD_WIDTH, FIELD_HEIGHT),
            self.surface_size.0,
            self.surface_size.1,
        )
    }
}

/// ペットを起動できない原因。
#[derive(Debug)]
pub enum PetError {
    Animal(vmc_pet_body::animal::AnimalError),
}

impl std::fmt::Display for PetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Animal(e) => write!(f, "failed to load the initial animal: {e}"),
        }
    }
}

impl std::error::Error for PetError {}

impl Surface for Pet {
    fn draw(&mut self, canvas: &mut [u8], width: u32, height: u32) {
        self.surface_size = (width, height);
        self.advance(Instant::now());

        // echo(入力の可視化)は体の時間とは独立に、描画のたびに更新する。
        // 体の場は書き換えないので、ホバーし続けても体には何の影響も無い。
        if let Some(at) = self.hovering_at {
            self.touch(Touch::Hover { at });
        }
        self.echo.decay(ECHO_DECAY_PER_FRAME);

        // 生物が場の端で分断されて見えないよう、表示原点を重心へ寄せる
        self.camera
            .follow(self.body.observe(), GRID_COLUMNS, GRID_ROWS);
        canvas.fill(0);
        self.grid.draw(
            self.body.observe(),
            self.echo.view(),
            self.camera.origin(),
            canvas,
            width,
            height,
        );
    }

    fn input_region(&self, width: u32, height: u32) -> InputRegion {
        let bounds = self.grid.bounds(width, height);
        InputRegion {
            x: bounds.x,
            y: bounds.y,
            width: bounds.width,
            height: bounds.height,
        }
    }

    fn on_pointer(&mut self, input: PointerInput) {
        match input {
            PointerInput::Entered { x, y } | PointerInput::Moved { x, y } => {
                self.hovering_at = self.cell_under(x, y);
            }
            PointerInput::Pressed { x, y } => {
                self.hovering_at = self.cell_under(x, y);
                if let Some(at) = self.hovering_at {
                    self.touch(Touch::Click { at });
                }
            }
            PointerInput::Left => {
                self.hovering_at = None;
                self.touch(Touch::Leave);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 描画を1回通して、サーフェスの大きさを Pet に教える。
    fn drawn_pet() -> Pet {
        let mut pet = Pet::new(DEFAULT_ANIMAL_CODE).unwrap();
        let mut canvas = vec![0u8; 384 * 384 * 4];
        pet.draw(&mut canvas, 384, 384);
        pet
    }

    fn mass(pet: &Pet) -> f32 {
        let view = pet.body.observe();
        let mut total = 0.0;
        for y in 0..view.height() {
            for x in 0..view.width() {
                total += view.get(x, y);
            }
        }
        total
    }

    #[test]
    fn input_region_matches_the_drawn_grid() {
        // Arrange
        let pet = Pet::new(DEFAULT_ANIMAL_CODE).unwrap();

        // Act
        let region = pet.input_region(384, 384);

        // Assert: 384px を 32 セルで割り切るのでサーフェス全体を覆う
        assert_eq!(region.x, 0);
        assert_eq!(region.y, 0);
        assert_eq!(region.width, 384);
        assert_eq!(region.height, 384);
    }

    #[test]
    fn advance_runs_one_step_per_interval() {
        // Arrange
        let mut pet = Pet::new(DEFAULT_ANIMAL_CODE).unwrap();
        let start = pet.last_step;
        let interval = pet.step_interval;

        // Act: 2間隔分だけ時刻を進める
        pet.advance(start + interval * 2);

        // Assert
        assert_eq!(pet.last_step, start + interval * 2);
    }

    #[test]
    fn advance_drops_the_backlog_when_it_falls_too_far_behind() {
        // Arrange
        let mut pet = Pet::new(DEFAULT_ANIMAL_CODE).unwrap();
        let start = pet.last_step;
        let interval = pet.step_interval;

        // Act: 上限を超える遅れを与える
        let now = start + interval * (MAX_CATCH_UP_STEPS + 10);
        pet.advance(now);

        // Assert: 追いつきを諦めて現在時刻に合わせる
        assert_eq!(pet.last_step, now);
    }

    #[test]
    fn being_left_alone_weakens_the_body_without_killing_it() {
        // Arrange
        let mut pet = Pet::new(DEFAULT_ANIMAL_CODE).unwrap();
        let healthy = mass(&pet);

        // Act: 一切触れずに20000ステップ(≈22分)進める
        for _ in 0..20_000 {
            let next = pet.last_step + pet.step_interval;
            pet.advance(next);
        }

        // Assert: 弱るが、崩壊はしない
        let neglected = mass(&pet);
        assert!(neglected > 40.0, "neglect must not kill the body, got {neglected}");
        assert!(
            neglected < healthy * 0.98,
            "neglect must measurably weaken the body; healthy={healthy} neglected={neglected}"
        );
    }

    #[test]
    fn clicking_a_neglected_pet_restores_its_vigor() {
        // Arrange: 放置して弱らせる
        let mut pet = drawn_pet();
        for _ in 0..20_000 {
            let next = pet.last_step + pet.step_interval;
            pet.advance(next);
        }
        let neglected = mass(&pet);

        // Act: クリックを繰り返して育て直す
        for _ in 0..30 {
            pet.on_pointer(PointerInput::Pressed { x: 192.0, y: 192.0 });
            for _ in 0..50 {
                let next = pet.last_step + pet.step_interval;
                pet.advance(next);
            }
        }

        // Assert
        assert!(
            mass(&pet) > neglected,
            "clicking must let the body recover; neglected={neglected} restored={}",
            mass(&pet)
        );
    }

    #[test]
    fn a_click_inside_the_grid_feeds_the_body() {
        // Arrange
        let mut pet = drawn_pet();
        let before = mass(&pet);

        // Act: サーフェスの中央を押す
        pet.on_pointer(PointerInput::Pressed { x: 192.0, y: 192.0 });

        // Assert
        assert!(mass(&pet) > before, "a click must inject energy");
    }

    #[test]
    fn a_click_also_lights_up_the_echo() {
        // Arrange
        let mut pet = drawn_pet();

        // Act
        pet.on_pointer(PointerInput::Pressed { x: 192.0, y: 192.0 });

        // Assert
        let at = pet.hovering_at.expect("the click must land inside the grid");
        assert!(pet.echo.view().get(at.x, at.y) > 0.0);
    }

    #[test]
    fn hovering_lights_the_echo_without_touching_the_body() {
        // Arrange
        let mut pet = drawn_pet();
        let before = mass(&pet);

        // Act: ポインタを乗せてから描画を1回通す(echo の更新は draw の中で起こる)
        pet.on_pointer(PointerInput::Entered { x: 192.0, y: 192.0 });
        let at = pet.hovering_at.expect("hovering over the grid must resolve a cell");
        let mut canvas = vec![0u8; 384 * 384 * 4];
        pet.draw(&mut canvas, 384, 384);

        // Assert: 体の総量は変わらないが、echo は光る
        assert_eq!(mass(&pet), before, "hovering must not touch the body");
        assert!(pet.echo.view().get(at.x, at.y) > 0.0, "hovering must light the echo");
    }

    #[test]
    fn a_pointer_outside_the_grid_touches_nothing() {
        // Arrange
        let mut pet = drawn_pet();
        let before = mass(&pet);

        // Act: グリッドの外を押す
        pet.on_pointer(PointerInput::Pressed { x: 1000.0, y: 1000.0 });

        // Assert
        assert_eq!(mass(&pet), before);
        assert!(pet.hovering_at.is_none());
    }

    #[test]
    fn leaving_stops_the_hover_tracking() {
        // Arrange
        let mut pet = drawn_pet();
        pet.on_pointer(PointerInput::Entered { x: 192.0, y: 192.0 });
        assert!(pet.hovering_at.is_some());

        // Act
        pet.on_pointer(PointerInput::Left);

        // Assert
        assert!(pet.hovering_at.is_none());
    }
}

#[cfg(test)]
mod resilience_tests {
    use super::*;
    use vmc_pet_body::{BodyPort, CellPos, Perturbation};

    /// 体の時間を `steps` ぶん進める。1間隔ずつ刻んで追いつき上限に掛からないようにする。
    fn run(pet: &mut Pet, steps: usize) {
        for _ in 0..steps {
            let next = pet.last_step + pet.step_interval;
            pet.advance(next);
        }
    }

    /// ポインタを動かしながら何度もクリックする、という操作を模す。
    /// ホバーは体に一切触れないため、体への負荷はクリックだけから来る。
    fn harass(pet: &mut Pet, seed: u64, rounds: usize) {
        let mut state = seed;
        let mut at = CellPos { x: 16, y: 16 };
        for round in 0..rounds {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            at = CellPos {
                x: (at.x + ((state >> 33) % 5) as usize) % FIELD_WIDTH,
                y: (at.y + ((state >> 13) % 5) as usize) % FIELD_HEIGHT,
            };
            if round % 3 == 0 {
                pet.touch(Touch::Click { at });
            }
            run(pet, 10);
        }
    }

    #[test]
    fn the_body_always_comes_back_after_being_harassed() {
        // Arrange / Act / Assert: 乱暴にクリックされても、必ず生きた状態へ戻る。
        // ホバーは体に触れないため、以前あった「ホバーとクリックの併用で安全境界が
        // 単調にならない」という問題(docs/DESIGN.md 参照)はここでは起こりえない。
        // それでも崩壊検知は防御として残す。
        for seed in [12345u64, 99, 777, 20260912] {
            let mut pet = Pet::new(DEFAULT_ANIMAL_CODE).unwrap();
            harass(&mut pet, seed, 40);
            run(&mut pet, 600);

            let mass = pet.body.mass();
            assert!(
                mass > 40.0,
                "the pet must recover after harassment (seed {seed}), got {mass}"
            );
        }
    }

    #[test]
    fn a_deliberately_scorched_field_is_revived() {
        // Arrange: 場じゅうに最大の摂動を撃ち込んで焼き払う
        let mut pet = Pet::new(DEFAULT_ANIMAL_CODE).unwrap();
        for _ in 0..12 {
            for y in (0..FIELD_HEIGHT).step_by(4) {
                for x in (0..FIELD_WIDTH).step_by(4) {
                    pet.body.inject(Perturbation {
                        at: CellPos { x, y },
                        radius: 6.0,
                        amount: 1.0,
                    });
                }
            }
            pet.body.step();
        }

        // Act: 崩壊を検知させる
        run(&mut pet, 900);

        // Assert
        assert!(pet.body.mass() > 40.0, "the collapsed body must be revived");
    }
}
