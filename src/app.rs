//! PC版のペット。世界モデル(体・入力の翻訳・崩壊検知・echo)は
//! `vmc_pet_body::Pet` が持つ。ここが持つのは PC 固有の事情だけ:
//! 「いつ進めるか」(std::time、追いつき方の方針)、「どう入力を受け取り、
//! どう描くか」(Wayland のポインタ座標・ドットマトリックス描画)、
//! CPU 負荷の読み取り。M5Stack版(m5stack-cores3/src/bin/main.rs)とは
//! この境界のところだけが違い、それ以外の挙動は完全に共有している。

use std::time::{Duration, Instant};

use vmc_pet_body::Pet as PetCore;
use crate::interface::MachineLoad;
use crate::persistence::{now_unix_seconds, MemoryStore};
use crate::render::{Camera, DotGrid};
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

/// 1回の描画でまとめて進めるステップ数の上限。
/// 復帰直後など大きく遅れた場合に、追いつこうとして固まるのを防ぐ。
/// (この上限そのものはPC版だけの追いつき方針。M5Stack版は毎回1ステップだけ
/// 進めて自然に遅れるという別の方針を採っている。docs/M5STACK.md 参照)
const MAX_CATCH_UP_STEPS: u32 = 4;

/// echo(入力の可視化)の、毎フレームの減衰率。
/// `interface::pointer.rs`(旧)にあった `HOVER_ECHO_AMOUNT_PER_FRAME` と
/// 組み合わさって定常値を決める(docs/DESIGN.md「入力の可視化を体の場から
/// 分離する」参照)。この値では、撫で続けたときに約4秒で 0.8 前後へ収束し、
/// 離すと1〜2秒ほどで消える。
const ECHO_DECAY_PER_FRAME: f32 = 0.90;

/// 記憶を書き出す間隔。
///
/// 終了時にまとめて保存する作りにはしていない。Wayland のクライアントは
/// コンポジタごと落ちることもあり、確実に終了処理が走るとは限らないため、
/// 定期的に書いておく方が「突然終わっても直前まで覚えている」状態に近づく。
const SAVE_INTERVAL: Duration = Duration::from_secs(30);

/// Lenia の場を体として持つペット。
pub struct Pet {
    core: PetCore,
    grid: DotGrid,
    camera: Camera,
    step_interval: Duration,
    last_step: Instant,
    /// 直近のサーフェスの大きさ。ポインタ座標を場のセルへ写すのに要る。
    surface_size: (u32, u32),
    /// 機械の CPU 負荷を「環境の厳しさ」として体に伝えるための読み取り役。
    machine_load: MachineLoad,
    /// プロセスをまたぐ記憶の読み書き役。
    memory_store: MemoryStore,
    last_saved: Instant,
}

impl Pet {
    /// `code` の生物を読み込んで体を作る。
    /// 生物データは実行ファイルに埋め込んであるため、読み込みに失敗するのは
    /// コードが存在しないか、データが壊れている場合だけで、その場合は起動を止める。
    pub fn new(code: &str) -> Result<Self, PetError> {
        Self::with_memory_store(code, MemoryStore::new())
    }

    /// 記憶の読み書き役を明示して作る。テストは
    /// `MemoryStore::disabled()` を渡し、実行環境に既にある保存ファイルに
    /// 左右されないようにする。
    pub fn with_memory_store(code: &str, memory_store: MemoryStore) -> Result<Self, PetError> {
        let animal = vmc_pet_body::load_animal(code).map_err(PetError::Animal)?;
        eprintln!(
            "vmc-pet: loaded {} ({}) R={} T={}",
            animal.name, animal.code, animal.params.radius, animal.params.time_divisor
        );

        let mut core = PetCore::new(animal, FIELD_WIDTH, FIELD_HEIGHT);
        // 前回の記憶があれば、離れていた時間ぶん弱った状態で目を覚ます
        // (docs/DESIGN.md「プロセスをまたぐ記憶」参照)。
        if let Some(saved) = memory_store.load() {
            let seconds_away = saved.seconds_away(now_unix_seconds());
            core.restore(saved.memory, seconds_away);
            eprintln!(
                "vmc-pet: resumed after {:.1}h away; energy {:.2} -> {:.2}",
                seconds_away / 3600.0,
                saved.memory.energy,
                core.energy()
            );
        }

        Ok(Self {
            core,
            grid: DotGrid::new(GRID_COLUMNS, GRID_ROWS),
            camera: Camera::new(),
            step_interval: Duration::from_secs_f64(1.0 / STEPS_PER_SECOND as f64),
            last_step: Instant::now(),
            surface_size: (0, 0),
            machine_load: MachineLoad::new(),
            memory_store,
            last_saved: Instant::now(),
        })
    }

    /// 前回のステップからの経過分だけ体を進める。
    fn advance(&mut self, now: Instant) {
        let mut steps = 0;
        while now.duration_since(self.last_step) >= self.step_interval && steps < MAX_CATCH_UP_STEPS
        {
            let energy_before = self.core.energy();
            let collapsed = self.core.step();
            // 機械が忙しいほど、環境が厳しくエネルギーが早く尽きるようにする
            self.core.apply_environmental_stress(self.machine_load.sample());
            if energy_before > 0.0 && self.core.energy() == 0.0 {
                eprintln!("vmc-pet: energy depleted; the body is weakening from neglect");
            }
            if collapsed {
                eprintln!("vmc-pet: the body collapsed; reviving");
            }
            self.last_step += self.step_interval;
            steps += 1;
        }
        if steps == MAX_CATCH_UP_STEPS {
            // 追いつけなかった分は捨てる
            self.last_step = now;
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
        let now = Instant::now();
        self.advance(now);

        if now.duration_since(self.last_saved) >= SAVE_INTERVAL {
            self.memory_store.save(self.core.memory());
            self.last_saved = now;
        }

        // echo(入力の可視化)は体の時間とは独立に、描画のたびに更新する。
        // 体の場は書き換えないので、ホバーし続けても体には何の影響も無い。
        self.core.tick_input(ECHO_DECAY_PER_FRAME);
        // 世話がいつ来るかを学ぶため、時刻を知らせる(体は時計を読まない)。
        self.core.tick_clock(crate::persistence::now_unix_seconds());

        // 生物が場の端で分断されて見えないよう、表示原点を重心へ寄せる
        self.camera
            .follow(self.core.observe(), GRID_COLUMNS, GRID_ROWS);
        canvas.fill(0);
        self.grid.draw(
            self.core.observe(),
            self.core.echo_view(),
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
                self.core.hover_at(self.cell_under(x, y));
            }
            PointerInput::Pressed { x, y } => match self.cell_under(x, y) {
                Some(at) => self.core.click(at),
                None => self.core.hover_at(None),
            },
            PointerInput::Left => {
                self.core.leave();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 記憶を読まないペット。実行環境に既にある保存ファイル(実際にペットを
    /// 動かすと作られる)にテストが左右されないよう、必ずこれを使う。
    fn fresh_pet() -> Pet {
        Pet::with_memory_store(DEFAULT_ANIMAL_CODE, MemoryStore::disabled()).unwrap()
    }

    /// 描画を1回通して、サーフェスの大きさを Pet に教える。
    fn drawn_pet() -> Pet {
        let mut pet = fresh_pet();
        let mut canvas = vec![0u8; 384 * 384 * 4];
        pet.draw(&mut canvas, 384, 384);
        pet
    }

    #[test]
    fn input_region_matches_the_drawn_grid() {
        // Arrange
        let pet = fresh_pet();

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
        let mut pet = fresh_pet();
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
        let mut pet = fresh_pet();
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
        let mut pet = fresh_pet();
        let healthy = pet.core.mass();

        // Act: 一切触れずに20000ステップ(≈22分)進める
        for _ in 0..20_000 {
            let next = pet.last_step + pet.step_interval;
            pet.advance(next);
        }

        // Assert: 弱るが、崩壊はしない
        let neglected = pet.core.mass();
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
        let neglected = pet.core.mass();

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
            pet.core.mass() > neglected,
            "clicking must let the body recover; neglected={neglected} restored={}",
            pet.core.mass()
        );
    }

    #[test]
    fn a_click_inside_the_grid_feeds_the_body() {
        // Arrange
        let mut pet = drawn_pet();
        let before = pet.core.mass();

        // Act: サーフェスの中央を押す
        pet.on_pointer(PointerInput::Pressed { x: 192.0, y: 192.0 });

        // Assert
        assert!(pet.core.mass() > before, "a click must inject energy");
    }

    #[test]
    fn a_click_also_lights_up_the_echo() {
        // Arrange
        let mut pet = drawn_pet();

        // Act
        pet.on_pointer(PointerInput::Pressed { x: 192.0, y: 192.0 });

        // Assert
        let at = pet.core.touching_at().expect("the click must land inside the grid");
        assert!(pet.core.echo_view().get(at.x, at.y) > 0.0);
    }

    #[test]
    fn hovering_lights_the_echo_without_touching_the_body() {
        // Arrange
        let mut pet = drawn_pet();
        let before = pet.core.mass();

        // Act: ポインタを乗せてから描画を1回通す(echo の更新は draw の中で起こる)
        pet.on_pointer(PointerInput::Entered { x: 192.0, y: 192.0 });
        let at = pet.core.touching_at().expect("hovering over the grid must resolve a cell");
        let mut canvas = vec![0u8; 384 * 384 * 4];
        pet.draw(&mut canvas, 384, 384);

        // Assert: 体の総量は変わらないが、echo は光る
        assert_eq!(pet.core.mass(), before, "hovering must not touch the body");
        assert!(pet.core.echo_view().get(at.x, at.y) > 0.0, "hovering must light the echo");
    }

    #[test]
    fn a_pointer_outside_the_grid_touches_nothing() {
        // Arrange
        let mut pet = drawn_pet();
        let before = pet.core.mass();

        // Act: グリッドの外を押す
        pet.on_pointer(PointerInput::Pressed { x: 1000.0, y: 1000.0 });

        // Assert
        assert_eq!(pet.core.mass(), before);
        assert!(pet.core.touching_at().is_none());
    }

    #[test]
    fn leaving_stops_the_hover_tracking() {
        // Arrange
        let mut pet = drawn_pet();
        pet.on_pointer(PointerInput::Entered { x: 192.0, y: 192.0 });
        assert!(pet.core.touching_at().is_some());

        // Act
        pet.on_pointer(PointerInput::Left);

        // Assert
        assert!(pet.core.touching_at().is_none());
    }

    /// 起動時に記憶を読んで復元する配線そのものを確かめる
    /// (保存 → 別プロセスとして起動し直す、を1つのテストで模す)。
    #[test]
    fn a_restarted_pet_wakes_up_with_the_saved_energy() {
        // Arrange: 弱った状態を保存しておく
        let path = std::env::temp_dir()
            .join(format!("vmc-pet-test-{}-restart", std::process::id()))
            .join("state.json");
        let mut store = MemoryStore::at(path.clone());
        store.save(vmc_pet_body::PetMemory { energy: 0.25 });

        // Act: その保存ファイルを読むペットを起動する
        let pet = Pet::with_memory_store(DEFAULT_ANIMAL_CODE, MemoryStore::at(path.clone()))
            .unwrap();

        // Assert: 満タン(1.0)ではなく、保存されていた値で目を覚ます。
        // 保存直後なので離れていた時間はごくわずかで、減衰も無視できる。
        let energy = pet.core.energy();
        assert!(
            (energy - 0.25).abs() < 0.01,
            "a restarted pet must remember how it was left, got {energy}"
        );

        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_pet_with_no_saved_memory_starts_fresh() {
        // Arrange / Act: 初回起動(保存ファイルが無い)
        let pet = fresh_pet();

        // Assert
        assert_eq!(pet.core.energy(), 1.0);
    }
}

#[cfg(test)]
mod resilience_tests {
    use super::*;
    use vmc_pet_body::{BodyPort, CellPos, Perturbation};

    /// 記憶を読まないペット(tests モジュールの同名関数と同じ理由)。
    fn fresh_pet() -> Pet {
        Pet::with_memory_store(DEFAULT_ANIMAL_CODE, MemoryStore::disabled()).unwrap()
    }

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
                pet.core.click(at);
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
            let mut pet = fresh_pet();
            harass(&mut pet, seed, 40);
            run(&mut pet, 600);

            let mass = pet.core.mass();
            assert!(
                mass > 40.0,
                "the pet must recover after harassment (seed {seed}), got {mass}"
            );
        }
    }

    #[test]
    fn a_deliberately_scorched_field_is_revived() {
        // Arrange: 場じゅうに最大の摂動を撃ち込んで焼き払う
        let mut pet = fresh_pet();
        for _ in 0..12 {
            for y in (0..FIELD_HEIGHT).step_by(4) {
                for x in (0..FIELD_WIDTH).step_by(4) {
                    pet.core.inject(Perturbation {
                        at: CellPos { x, y },
                        radius: 6.0,
                        amount: 1.0,
                    });
                }
            }
            pet.core.step();
        }

        // Act: 崩壊を検知させる
        run(&mut pet, 900);

        // Assert
        assert!(pet.core.mass() > 40.0, "the collapsed body must be revived");
    }
}
