//! 【実験】多チャンネル Lenia の生物を、PC 版の窓で見て確かめる(`--preview-multichannel <id>`)。
//!
//! ペットの仕組み(エネルギー・慣れ・学習・色素・コントローラ・記憶)にはつながず、体だけを動かす。
//! 場は、ペットの試験一式にかけたときと同じ 64×64・R 13(docs/experiments/rule-candidates.md
//! 「多チャンネルの生物をペットの試験にかける」)。進め方(1秒に 15 ステップ、遅れたら捨てる)と
//! クリックの強さはペットと同じにし、クリックは全チャンネルへ同じ量を注入する。崩壊したら置き直す。

use std::time::{Duration, Instant};

use crate::render::{Camera, DotGrid};
use crate::shell::{InputRegion, PointerInput, Surface};
use vmc_pet_body::multichannel::{load_multichannel, MultiAnimal, MultiWorld};
use vmc_pet_body::{accumulate_into, body_perturbation_for, Field, Touch};

/// 場の一辺と、縮めた後の R(試験一式と同じ)。
const FIELD_SIZE: usize = 64;
const RADIUS: usize = 13;

/// 表示のドットの数。場と 1:1 にする。
const GRID_SIZE: usize = 64;

/// ペットと同じ進め方(`app.rs`)。
const STEPS_PER_SECOND: u32 = 15;
const MAX_CATCH_UP_STEPS: u32 = 4;

/// プレビューを起動できない原因。
#[derive(Debug)]
pub enum PreviewError {
    Parse(serde_json::Error),
    UnknownId(String),
    DoesNotFit(String),
}

impl std::fmt::Display for PreviewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(error) => write!(
                f,
                "failed to read the bundled multichannel animals: {error}"
            ),
            Self::UnknownId(id) => write!(f, "unknown multichannel animal: {id}"),
            Self::DoesNotFit(id) => {
                write!(f, "{id} does not fit a {FIELD_SIZE}x{FIELD_SIZE} field")
            }
        }
    }
}

impl std::error::Error for PreviewError {}

/// 多チャンネルの生物を表示するだけのサーフェス。
pub struct MultichannelPreview {
    animal: MultiAnimal,
    world: MultiWorld,
    grid: DotGrid,
    camera: Camera,
    step_interval: Duration,
    last_step: Instant,
    surface_size: (u32, u32),
}

impl MultichannelPreview {
    pub fn new(id: &str) -> Result<Self, PreviewError> {
        let animal = load_multichannel(id)
            .map_err(PreviewError::Parse)?
            .ok_or_else(|| PreviewError::UnknownId(id.to_string()))?;
        let world = MultiWorld::place(&animal, FIELD_SIZE, RADIUS)
            .ok_or_else(|| PreviewError::DoesNotFit(id.to_string()))?;
        eprintln!(
            "vmc-pet: previewing multichannel animal {} {} ({} channels, {} kernels) on {FIELD_SIZE}x{FIELD_SIZE} at R={RADIUS}; no energy, learning or memory",
            animal.id,
            animal.name,
            animal.cells.len(),
            animal.params.len()
        );
        Ok(Self {
            animal,
            world,
            grid: DotGrid::new(GRID_SIZE, GRID_SIZE),
            camera: Camera::new(),
            step_interval: Duration::from_secs_f64(1.0 / STEPS_PER_SECOND as f64),
            last_step: Instant::now(),
            surface_size: (0, 0),
        })
    }

    fn advance(&mut self, now: Instant) {
        let mut steps = 0;
        while now.duration_since(self.last_step) >= self.step_interval && steps < MAX_CATCH_UP_STEPS
        {
            self.world.step();
            if self.world.collapsed() {
                eprintln!("vmc-pet: the multichannel body collapsed; placing it again");
                if let Some(world) = MultiWorld::place(&self.animal, FIELD_SIZE, RADIUS) {
                    self.world = world;
                }
            }
            self.last_step += self.step_interval;
            steps += 1;
        }
        if steps == MAX_CATCH_UP_STEPS {
            self.last_step = now;
        }
    }

    /// 全チャンネルの和を1枚の場にしたもの。表示原点を重心へ寄せる(`Camera`)ためだけに使う。
    fn summed_field(&self) -> Field {
        let mut field = Field::new(FIELD_SIZE, FIELD_SIZE);
        field.map(|x, y, _| self.world.total(y * FIELD_SIZE + x));
        field
    }
}

impl Surface for MultichannelPreview {
    fn draw(&mut self, canvas: &mut [u8], width: u32, height: u32) {
        self.surface_size = (width, height);
        self.advance(Instant::now());
        let summed = self.summed_field();
        self.camera.follow(summed.view(), GRID_SIZE, GRID_SIZE);
        canvas.fill(0);
        self.grid.draw_channels(
            &self.world.channels,
            FIELD_SIZE,
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
        let PointerInput::Pressed { x, y } = input else {
            return;
        };
        let Some(at) = self.grid.cell_at(
            (x, y),
            self.camera.origin(),
            (FIELD_SIZE, FIELD_SIZE),
            self.surface_size.0,
            self.surface_size.1,
        ) else {
            return;
        };
        let Some(perturbation) = body_perturbation_for(Touch::Click { at }) else {
            return;
        };
        for channel in &mut self.world.channels {
            accumulate_into(channel, FIELD_SIZE, FIELD_SIZE, &perturbation, 0.0, 1.0);
        }
    }
}
