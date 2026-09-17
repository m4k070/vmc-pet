//! 【実験】粒子の体(`vmc_pet_body::particles`)を、PC 版の窓で動かして見る。
//!
//! 探索で見つけた候補を番号で選ぶ(docs/experiments/body-candidates.md「閉じた場を動き回る粒子の体を探す」)。
//! ペットの仕組み(エネルギー・慣れ・色素・学習・記憶)にはつながない。
//!
//! - 表示はペットと同じ 32×32 のドット。粒子を種類ごとのチャンネルに振り分け、近くのセルへ配って描く
//!   (種類 0/1/2 を赤/緑/青。`DotGrid::draw_channels`)
//! - `zoom` 倍に拡大して描く。物理の箱の一辺は 32 / `zoom` セルになり、体は大きく見えるが、動き回れる広さは
//!   狭くなる。探索と試験は `zoom` = 1(箱 32 セル)で行った
//! - ポインタを乗せると、その点へ粒子を誘う(強さ 0.02。1937 は強く誘うと止まったままになるため)。クリックすると、その点から
//!   半径 4.5 ドット(クリックの半径)の粒子を外向きに弾く(試験で戻れた強さ 1.0)
//! - 場の端は描かない。体は窓の端で跳ね返る

use std::time::{Duration, Instant};

use crate::render::DotGrid;
use crate::shell::{InputRegion, PointerInput, Surface};
use vmc_pet_body::particles::{ParticleParams, ParticleWorld, DEFAULT_FIELD_SIZE};

/// 表示のドットの数(ペットと同じ)。
const GRID_SIZE: usize = 32;

/// ペットと同じ進め方(`app.rs`)。
const STEPS_PER_SECOND: u32 = 15;
const MAX_CATCH_UP_STEPS: u32 = 4;

/// 誘いの強さ。1091 は導けるが、1937 を止めにくい弱さ(docs/experiments/body-candidates.md「誘いの強さ」)。
const LURE_STRENGTH: f32 = 0.02;
/// クリックで弾く半径(ドット)と速さ。半径はクリックの摂動と同じ、速さは試験で戻れた強さ。
const POKE_RADIUS_DOTS: f32 = 4.5;
const POKE_IMPULSE: f32 = 1.0;
/// 粒子1個が配る値。塊の中心が濃く、ばらけた粒子は薄く見える程度。
const PARTICLE_WEIGHT: f32 = 0.35;

/// 粒子の体を動かすサーフェス。
pub struct ParticlePreview {
    world: ParticleWorld,
    zoom: f32,
    grid: DotGrid,
    channels: Vec<Vec<f32>>,
    step_interval: Duration,
    last_step: Instant,
    surface_size: (u32, u32),
}

impl ParticlePreview {
    pub fn new(seed: u64, zoom: u32) -> Self {
        let zoom = zoom.max(1) as f32;
        let params = ParticleParams::from_seed(seed);
        eprintln!(
            "vmc-pet: previewing particle body #{seed} ({} types x {} particles, r_max {:.2}, force {:.3}, friction {:.2}) in a {:.0}-cell box drawn at x{zoom}; hover to lure, click to poke",
            params.types,
            params.per_type,
            params.r_max,
            params.force,
            params.friction,
            DEFAULT_FIELD_SIZE / zoom
        );
        let types = params.types;
        Self {
            world: ParticleWorld::new(params, DEFAULT_FIELD_SIZE / zoom, 1),
            zoom,
            grid: DotGrid::new(GRID_SIZE, GRID_SIZE),
            channels: vec![vec![0.0; GRID_SIZE * GRID_SIZE]; types.min(3)],
            step_interval: Duration::from_secs_f64(1.0 / STEPS_PER_SECOND as f64),
            last_step: Instant::now(),
            surface_size: (0, 0),
        }
    }

    fn advance(&mut self, now: Instant) {
        let mut steps = 0;
        while now.duration_since(self.last_step) >= self.step_interval && steps < MAX_CATCH_UP_STEPS
        {
            self.world.step();
            self.last_step += self.step_interval;
            steps += 1;
        }
        if steps == MAX_CATCH_UP_STEPS {
            self.last_step = now;
        }
    }

    /// 粒子を、種類ごとのチャンネルの近くの4セルへ距離に応じて配る。
    fn rasterize(&mut self) {
        for channel in &mut self.channels {
            channel.iter_mut().for_each(|value| *value = 0.0);
        }
        let last = (GRID_SIZE - 1) as f32;
        for i in 0..self.world.x.len() {
            let Some(channel) = self.channels.get_mut(self.world.kind[i]) else {
                continue;
            };
            let u = (self.world.x[i] * self.zoom - 0.5).clamp(0.0, last);
            let v = (self.world.y[i] * self.zoom - 0.5).clamp(0.0, last);
            let (left, top) = (u.floor() as usize, v.floor() as usize);
            let (right, bottom) = ((left + 1).min(GRID_SIZE - 1), (top + 1).min(GRID_SIZE - 1));
            let (fx, fy) = (u - left as f32, v - top as f32);
            for (x, y, share) in [
                (left, top, (1.0 - fx) * (1.0 - fy)),
                (right, top, fx * (1.0 - fy)),
                (left, bottom, (1.0 - fx) * fy),
                (right, bottom, fx * fy),
            ] {
                let cell = &mut channel[y * GRID_SIZE + x];
                *cell = (*cell + PARTICLE_WEIGHT * share).min(1.0);
            }
        }
    }

    /// サーフェス上の座標を、物理の箱の座標に直す。グリッドの外なら `None`。
    fn to_world(&self, x: f64, y: f64) -> Option<(f32, f32)> {
        let bounds = self.grid.bounds(self.surface_size.0, self.surface_size.1);
        if bounds.width == 0 {
            return None;
        }
        let cell = bounds.width as f32 / GRID_SIZE as f32;
        let u = (x as f32 - bounds.x as f32) / cell;
        let v = (y as f32 - bounds.y as f32) / cell;
        let inside = (0.0..GRID_SIZE as f32).contains(&u) && (0.0..GRID_SIZE as f32).contains(&v);
        inside.then_some((u / self.zoom, v / self.zoom))
    }
}

impl Surface for ParticlePreview {
    fn draw(&mut self, canvas: &mut [u8], width: u32, height: u32) {
        self.surface_size = (width, height);
        self.advance(Instant::now());
        self.rasterize();
        canvas.fill(0);
        self.grid
            .draw_channels(&self.channels, GRID_SIZE, (0.0, 0.0), canvas, width, height);
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
                self.world.lure = self.to_world(x, y).map(|(px, py)| (px, py, LURE_STRENGTH));
            }
            PointerInput::Pressed { x, y } => {
                if let Some((px, py)) = self.to_world(x, y) {
                    self.world
                        .poke(px, py, POKE_RADIUS_DOTS / self.zoom, POKE_IMPULSE);
                }
            }
            PointerInput::Left => self.world.lure = None,
        }
    }
}
