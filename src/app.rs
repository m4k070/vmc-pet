//! Lenia の場を体として持つペット本体。
//! 場の更新レートと描画レートを分離し、描画時に経過分だけステップを進める。

use std::time::{Duration, Instant};

use crate::body::{load_animal, Field, Lenia};
use crate::render::{Camera, DotGrid};
use crate::shell::{InputRegion, PointerInput, Surface};

/// 場の解像度。表示と 1:1 にしてある。Orbium は 20x20 なので画面の 6 割強を占める。
/// この大きさでも生物の挙動が変わらないことは実測で確かめた(docs/DESIGN.md 参照)。
const FIELD_WIDTH: usize = 32;
const FIELD_HEIGHT: usize = 32;

/// 表示解像度。場を平均プーリングで落として描く。
const GRID_COLUMNS: usize = 32;
const GRID_ROWS: usize = 32;

/// 起動時に読み込む生物。assets/animals.json のコードを指す。
const INITIAL_ANIMAL_CODE: &str = "O2u";

/// 場を進める頻度。描画レートとは独立に決める。
const STEPS_PER_SECOND: u32 = 15;

/// 1回の描画でまとめて進めるステップ数の上限。
/// 復帰直後など大きく遅れた場合に、追いつこうとして固まるのを防ぐ。
const MAX_CATCH_UP_STEPS: u32 = 4;

/// Lenia の場を体として持つペット。
pub struct Pet {
    field: Field,
    lenia: Lenia,
    grid: DotGrid,
    camera: Camera,
    step_interval: Duration,
    last_step: Instant,
}

impl Pet {
    /// 生物を読み込んで場の中央に配置する。
    /// 生物データは実行ファイルに埋め込んであるため、読み込みに失敗するのは
    /// データが壊れている場合だけで、その場合は起動を止める。
    pub fn new() -> Result<Self, PetError> {
        let animal = load_animal(INITIAL_ANIMAL_CODE).map_err(PetError::Animal)?;
        eprintln!(
            "vmc-pet: loaded {} ({}) R={} T={}",
            animal.name, animal.code, animal.params.radius, animal.params.time_divisor
        );

        let mut field = Field::new(FIELD_WIDTH, FIELD_HEIGHT);
        field.place_centered(&animal.pattern);

        Ok(Self {
            field,
            lenia: Lenia::new(animal.params),
            grid: DotGrid::new(GRID_COLUMNS, GRID_ROWS),
            camera: Camera::new(),
            step_interval: Duration::from_secs_f64(1.0 / STEPS_PER_SECOND as f64),
            last_step: Instant::now(),
        })
    }

    /// 前回のステップからの経過分だけ場を進める。
    fn advance(&mut self, now: Instant) {
        let mut steps = 0;
        while now.duration_since(self.last_step) >= self.step_interval && steps < MAX_CATCH_UP_STEPS
        {
            self.lenia.step(&mut self.field);
            self.last_step += self.step_interval;
            steps += 1;
        }
        if steps == MAX_CATCH_UP_STEPS {
            // 追いつけなかった分は捨てる
            self.last_step = now;
        }
    }
}

/// ペットを起動できない原因。
#[derive(Debug)]
pub enum PetError {
    Animal(crate::body::animal::AnimalError),
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
        self.advance(Instant::now());
        // 生物が場の端で分断されて見えないよう、表示原点を重心へ寄せる
        self.camera.follow(self.field.view(), GRID_COLUMNS, GRID_ROWS);
        canvas.fill(0);
        self.grid
            .draw(self.field.view(), self.camera.origin(), canvas, width, height);
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
        // step 4 でここから場への摂動注入につなぐ
        match input {
            PointerInput::Pressed { x, y } => {
                eprintln!("vmc-pet: pointer pressed at ({x:.1}, {y:.1})");
            }
            PointerInput::Entered { .. } | PointerInput::Moved { .. } | PointerInput::Left => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_region_matches_the_drawn_grid() {
        // Arrange
        let pet = Pet::new().unwrap();

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
        let mut pet = Pet::new().unwrap();
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
        let mut pet = Pet::new().unwrap();
        let start = pet.last_step;
        let interval = pet.step_interval;

        // Act: 上限を超える遅れを与える
        let now = start + interval * (MAX_CATCH_UP_STEPS + 10);
        pet.advance(now);

        // Assert: 追いつきを諦めて現在時刻に合わせる
        assert_eq!(pet.last_step, now);
    }
}
