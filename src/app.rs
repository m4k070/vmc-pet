//! 体のプレースホルダ。場をダミー波形で満たして描画経路を確かめる。
//! step 3 で `fill_with` の中身を Lenia の更新規則に差し替える。

use std::f32::consts::TAU;
use std::time::Instant;

use crate::body::Field;
use crate::render::DotGrid;
use crate::shell::{InputRegion, PointerInput, Surface};

/// 場の解像度。Lenia の kernel radius R=12 を成立させるため表示より高くとる。
const FIELD_WIDTH: usize = 96;
const FIELD_HEIGHT: usize = 96;

/// 表示解像度。場を平均プーリングで落として描く。
const GRID_COLUMNS: usize = 32;
const GRID_ROWS: usize = 32;

/// ダミー波形の形状。step 3 で Lenia に置き換わる暫定値。
const WAVE_PERIOD_CELLS: f32 = 22.0;
const WAVE_SPEED_RADIANS_PER_SEC: f32 = 2.4;

/// 通常時と、ポインタが触れているときの波の振幅。
const IDLE_AMPLITUDE: f32 = 0.55;
const TOUCHED_AMPLITUDE: f32 = 1.0;

/// 場の中心からの減衰が 0 になる距離の、場の半径に対する割合。
/// DESIGN.md の周辺減衰(生物を画面外へ逃がさない境界条件)の先取り。
const FALLOFF_RADIUS_RATIO: f32 = 0.95;

/// ポインタが体に触れているか。step 4 で Touch enum に発展させる。
pub struct Pet {
    field: Field,
    grid: DotGrid,
    started_at: Instant,
    is_touched: bool,
}

impl Pet {
    pub fn new() -> Self {
        Self {
            field: Field::new(FIELD_WIDTH, FIELD_HEIGHT),
            grid: DotGrid::new(GRID_COLUMNS, GRID_ROWS),
            started_at: Instant::now(),
            is_touched: false,
        }
    }

    fn amplitude(&self) -> f32 {
        if self.is_touched {
            return TOUCHED_AMPLITUDE;
        }
        IDLE_AMPLITUDE
    }
}

impl Default for Pet {
    fn default() -> Self {
        Self::new()
    }
}

impl Surface for Pet {
    fn draw(&mut self, canvas: &mut [u8], width: u32, height: u32) {
        let elapsed = self.started_at.elapsed().as_secs_f32();
        let amplitude = self.amplitude();
        self.field
            .fill_with(|x, y| dummy_wave(x, y, elapsed, amplitude));

        canvas.fill(0);
        self.grid.draw(self.field.view(), canvas, width, height);
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
            PointerInput::Entered { x, y } => {
                eprintln!("vmc-pet: pointer entered at ({x:.1}, {y:.1})");
                self.is_touched = true;
            }
            PointerInput::Moved { .. } => {
                self.is_touched = true;
            }
            PointerInput::Pressed { x, y } => {
                eprintln!("vmc-pet: pointer pressed at ({x:.1}, {y:.1})");
                self.is_touched = true;
            }
            PointerInput::Left => {
                eprintln!("vmc-pet: pointer left");
                self.is_touched = false;
            }
        }
    }
}

/// 中心から広がる同心円状の進行波。周縁は減衰させて丸い塊に見せる。
/// 時刻だけに依存する純粋関数なので、描画レートを変えても見た目の速度は変わらない。
fn dummy_wave(x: usize, y: usize, elapsed_secs: f32, amplitude: f32) -> f32 {
    let center_x = FIELD_WIDTH as f32 / 2.0;
    let center_y = FIELD_HEIGHT as f32 / 2.0;
    let dx = x as f32 + 0.5 - center_x;
    let dy = y as f32 + 0.5 - center_y;
    let distance = (dx * dx + dy * dy).sqrt();

    let phase = distance / WAVE_PERIOD_CELLS * TAU - elapsed_secs * WAVE_SPEED_RADIANS_PER_SEC;
    let wave = 0.5 + 0.5 * phase.cos();

    let falloff_radius = center_x.min(center_y) * FALLOFF_RADIUS_RATIO;
    let falloff = (1.0 - distance / falloff_radius).clamp(0.0, 1.0);

    wave * falloff * amplitude
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dummy_wave_stays_within_the_normalized_range() {
        // Arrange
        let samples = [(0, 0), (48, 48), (95, 95), (10, 80)];

        // Act / Assert
        for elapsed in [0.0, 0.37, 5.0] {
            for (x, y) in samples {
                let value = dummy_wave(x, y, elapsed, TOUCHED_AMPLITUDE);
                assert!(
                    (0.0..=1.0).contains(&value),
                    "value {value} out of range at ({x}, {y}) t={elapsed}"
                );
            }
        }
    }

    #[test]
    fn dummy_wave_is_zero_at_the_field_corners() {
        // Arrange: 隅は減衰半径の外側にある

        // Act
        let value = dummy_wave(0, 0, 1.0, TOUCHED_AMPLITUDE);

        // Assert
        assert_eq!(value, 0.0);
    }

    #[test]
    fn input_region_matches_the_drawn_grid() {
        // Arrange
        let pet = Pet::new();

        // Act
        let region = pet.input_region(384, 384);

        // Assert: 384px を 32 セルで割り切るのでサーフェス全体を覆う
        assert_eq!(region.x, 0);
        assert_eq!(region.y, 0);
        assert_eq!(region.width, 384);
        assert_eq!(region.height, 384);
    }
}
