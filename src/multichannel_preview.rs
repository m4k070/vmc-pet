//! 【実験】多チャンネル Lenia の生物を、PC 版の窓で動かす。
//!
//! 2つのモードがある。
//!
//! - **表示だけ**(`--preview-multichannel <id>`): ペットの仕組みにはつながず、体だけを動かして見る。
//!   エネルギーは満タンのまま、テンポは 1.0
//! - **体としてつなぐ**(`--multichannel <id>`): いまのペットの仕組みのうち、元気(放置で弱る・CPU 負荷で
//!   速く尽きる・クリックで回復する)とテンポとクリックだけをつなぐ(`vmc_pet_body::multichannel::MultiBody`)。
//!   テンポは、`--preview-mood` で気分を固定したときだけ `Mood` の式で変わる(学習はつないでいないため)
//!
//! 場は、ペットの試験一式・放置の試験と同じ 64×64・R 13(docs/experiments/rule-candidates.md)。
//! 進め方(1秒に 15 ステップ、遅れたら捨てる)とクリックの強さはペットと同じにし、クリックは全チャンネルへ
//! 同じ量を注入する。崩壊したら置き直す。慣れ・色素・学習・自律コントローラ・記憶はつないでいない。

use std::time::{Duration, Instant};

use crate::interface::MachineLoad;
use crate::render::{Camera, DotGrid};
use crate::shell::{InputRegion, PointerInput, Surface};
use vmc_pet_body::multichannel::{load_multichannel, MultiBody};
use vmc_pet_body::{Field, Mood, MoodState};

/// 場の一辺と、縮めた後の R(試験一式と同じ)。
const FIELD_SIZE: usize = 64;
const RADIUS: usize = 13;

/// 表示のドットの数。場と 1:1 にする。
const GRID_SIZE: usize = 64;

/// ペットと同じ進め方(`app.rs`)。
const STEPS_PER_SECOND: u32 = 15;
const MAX_CATCH_UP_STEPS: u32 = 4;

/// 起動できない原因。
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

/// どこまでペットの仕組みにつなぐか。
enum Mode {
    /// 表示だけ。エネルギーは満タンのまま。
    Display,
    /// 元気・テンポ・クリックをつなぐ。
    Body {
        /// 気分を固定したときだけ持つ。テンポはここから決まる。
        mood: Option<Mood>,
        machine_load: MachineLoad,
    },
}

/// 多チャンネルの生物を動かすサーフェス。
pub struct MultichannelSurface {
    body: MultiBody,
    mode: Mode,
    grid: DotGrid,
    camera: Camera,
    step_interval: Duration,
    last_step: Instant,
    surface_size: (u32, u32),
}

impl MultichannelSurface {
    /// 表示だけのモード(`--preview-multichannel`)。
    pub fn display(id: &str) -> Result<Self, PreviewError> {
        let surface = Self::with_mode(id, Mode::Display)?;
        eprintln!(
            "vmc-pet: previewing multichannel animal {} {} ({} channels, {} kernels) on {FIELD_SIZE}x{FIELD_SIZE} at R={RADIUS}; no energy, learning or memory",
            surface.body.animal().id,
            surface.body.animal().name,
            surface.body.animal().cells.len(),
            surface.body.animal().params.len()
        );
        Ok(surface)
    }

    /// 元気・テンポ・クリックをつなぐモード(`--multichannel`)。`preview` があれば気分を固定する。
    pub fn body(id: &str, preview: Option<MoodState>) -> Result<Self, PreviewError> {
        let mood = preview.map(|state| {
            let mut mood = Mood::new();
            let (anticipation, disappointment) = state.anticipation_and_disappointment();
            mood.pin(anticipation, disappointment);
            mood
        });
        let surface = Self::with_mode(
            id,
            Mode::Body {
                mood,
                machine_load: MachineLoad::new(),
            },
        )?;
        eprintln!(
            "vmc-pet: running multichannel animal {} {} ({} channels, {} kernels) on {FIELD_SIZE}x{FIELD_SIZE} at R={RADIUS}; connected: energy, tempo{}, clicks; not connected: habituation, pigment, learning, controller, memory",
            surface.body.animal().id,
            surface.body.animal().name,
            surface.body.animal().cells.len(),
            surface.body.animal().params.len(),
            preview.map_or(String::new(), |state| format!(" (mood pinned to {})", state.name()))
        );
        Ok(surface)
    }

    fn with_mode(id: &str, mode: Mode) -> Result<Self, PreviewError> {
        let animal = load_multichannel(id)
            .map_err(PreviewError::Parse)?
            .ok_or_else(|| PreviewError::UnknownId(id.to_string()))?;
        let body = MultiBody::new(animal, FIELD_SIZE, RADIUS)
            .ok_or_else(|| PreviewError::DoesNotFit(id.to_string()))?;
        Ok(Self {
            body,
            mode,
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
            match &self.mode {
                Mode::Display => self.body.refill_energy(),
                Mode::Body { mood, .. } => {
                    // がっかりしているほど体の時間がゆっくり進む(`Pet::step` と同じ)
                    self.body.set_tempo(mood.as_ref().map_or(1.0, Mood::tempo));
                }
            }
            let energy_before = self.body.energy();
            let collapsed = self.body.step();
            if let Mode::Body { machine_load, .. } = &mut self.mode {
                // 機械が忙しいほど、環境が厳しくエネルギーが早く尽きる(`app.rs` と同じ)
                self.body.apply_environmental_stress(machine_load.sample());
                if energy_before > 0.0 && self.body.energy() == 0.0 {
                    eprintln!("vmc-pet: energy depleted; the body is weakening from neglect");
                }
            }
            if collapsed {
                eprintln!("vmc-pet: the multichannel body collapsed; placing it again");
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
        let world = self.body.world();
        let mut field = Field::new(FIELD_SIZE, FIELD_SIZE);
        field.map(|x, y, _| world.total(y * FIELD_SIZE + x));
        field
    }
}

impl Surface for MultichannelSurface {
    fn draw(&mut self, canvas: &mut [u8], width: u32, height: u32) {
        self.surface_size = (width, height);
        self.advance(Instant::now());
        let summed = self.summed_field();
        self.camera.follow(summed.view(), GRID_SIZE, GRID_SIZE);
        canvas.fill(0);
        self.grid.draw_channels(
            &self.body.world().channels,
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
        self.body.click(at);
    }
}
