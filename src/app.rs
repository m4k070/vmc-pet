//! step 1 時点の体のプレースホルダ。
//! ドットグリッド描画(step 2)と Lenia 場(step 3)で中身を差し替える。

use crate::shell::{InputRegion, PointerInput, Surface};

/// サーフェス端から体までの余白(論理ピクセル)。
/// この余白部分は完全透過かつ入力領域外になり、クリックが下のウィンドウへ抜ける。
const BODY_INSET: i32 = 64;

/// 余白が占めてよい各辺の割合の上限の逆数。
/// 小さいサーフェスを configure されても体が消えないよう、余白側を先に縮める。
const MIN_BODY_FRACTION_DIVISOR: i32 = 4;

/// 通常時と、ポインタが乗っているときの体の不透明度。
const IDLE_OPACITY: f32 = 0.35;
const TOUCHED_OPACITY: f32 = 0.65;

const BODY_RED: f32 = 0.35;
const BODY_GREEN: f32 = 0.85;
const BODY_BLUE: f32 = 0.80;

/// ポインタが体に触れているかどうか。step 4 で Touch enum に発展させる。
pub struct Pet {
    is_touched: bool,
}

impl Pet {
    pub fn new() -> Self {
        Self { is_touched: false }
    }

    fn opacity(&self) -> f32 {
        if self.is_touched {
            return TOUCHED_OPACITY;
        }
        IDLE_OPACITY
    }
}

impl Default for Pet {
    fn default() -> Self {
        Self::new()
    }
}

impl Surface for Pet {
    fn draw(&mut self, canvas: &mut [u8], width: u32, height: u32) {
        canvas.fill(0);

        let region = self.input_region(width, height);
        let alpha = self.opacity();
        // wl_shm の Argb8888 は premultiplied alpha を要求する
        let pixel = premultiplied_argb8888(BODY_RED, BODY_GREEN, BODY_BLUE, alpha);

        for y in region.y..region.y + region.height {
            let row_start = (y as usize * width as usize + region.x as usize) * 4;
            let row_end = row_start + region.width as usize * 4;
            for chunk in canvas[row_start..row_end].chunks_exact_mut(4) {
                chunk.copy_from_slice(&pixel);
            }
        }
    }

    fn input_region(&self, width: u32, height: u32) -> InputRegion {
        body_rect(width, height)
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

/// 余白を取った後の体の矩形を返す。
/// サーフェスが余白の2倍より小さい場合は余白側を縮め、体のサイズが負にならないようにする。
fn body_rect(width: u32, height: u32) -> InputRegion {
    let inset_x = BODY_INSET.min(width as i32 / MIN_BODY_FRACTION_DIVISOR);
    let inset_y = BODY_INSET.min(height as i32 / MIN_BODY_FRACTION_DIVISOR);
    InputRegion {
        x: inset_x,
        y: inset_y,
        width: width as i32 - inset_x * 2,
        height: height as i32 - inset_y * 2,
    }
}

/// 0.0〜1.0 の色とアルファを premultiplied ARGB8888 の1ピクセル分に変換する。
/// リトルエンディアン環境では u32 0xAARRGGBB がバイト列 [B, G, R, A] になる。
fn premultiplied_argb8888(red: f32, green: f32, blue: f32, alpha: f32) -> [u8; 4] {
    let to_byte = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
    [
        to_byte(blue * alpha),
        to_byte(green * alpha),
        to_byte(red * alpha),
        to_byte(alpha),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn premultiplied_argb8888_multiplies_each_channel_by_alpha() {
        // Arrange
        let alpha = 0.5;

        // Act
        let pixel = premultiplied_argb8888(1.0, 0.0, 1.0, alpha);

        // Assert
        assert_eq!(pixel, [128, 0, 128, 128]);
    }

    #[test]
    fn input_region_leaves_the_inset_as_click_through_area() {
        // Arrange
        let pet = Pet::new();

        // Act
        let region = pet.input_region(384, 384);

        // Assert
        assert_eq!(region.x, BODY_INSET);
        assert_eq!(region.width, 384 - BODY_INSET * 2);
    }

    #[test]
    fn body_rect_never_goes_negative_on_a_small_surface() {
        // Arrange: 余白の2倍より小さいサーフェスを configure された場合
        let width = 32;
        let height = 32;

        // Act
        let region = body_rect(width, height);

        // Assert
        assert!(region.width > 0, "body width must stay positive");
        assert!(region.height > 0, "body height must stay positive");
        assert!(region.x + region.width <= width as i32);
        assert!(region.y + region.height <= height as i32);
    }

    #[test]
    fn draw_leaves_the_inset_fully_transparent() {
        // Arrange
        let mut pet = Pet::new();
        let width = 128;
        let height = 128;
        let mut canvas = vec![0xffu8; width * height * 4];

        // Act
        pet.draw(&mut canvas, width as u32, height as u32);

        // Assert: 左上隅は余白なので透過、中心は体なので不透明
        assert_eq!(&canvas[0..4], &[0, 0, 0, 0]);
        let center = (height / 2 * width + width / 2) * 4;
        assert_ne!(canvas[center + 3], 0);
    }
}
