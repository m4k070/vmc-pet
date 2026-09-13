//! wlr-layer-shell によるデスクトップ常駐サーフェス。
//!
//! niri はスクロール型タイリングのため通常の xdg-toplevel はタイル配置される。
//! 「透過・枠なし・最前面」を満たせるのは layer-shell surface のみ。

use std::time::{Duration, Instant};

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_layer, delegate_output, delegate_pointer, delegate_registry,
    delegate_seat, delegate_shm,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        pointer::{PointerEvent, PointerEventKind, PointerHandler},
        Capability, SeatHandler, SeatState,
    },
    shell::{
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
        WaylandSurface,
    },
    shm::{slot::SlotPool, Shm, ShmHandler},
};
use wayland_client::{
    delegate_noop,
    globals::registry_queue_init,
    protocol::{wl_output, wl_pointer, wl_region, wl_seat, wl_shm, wl_surface},
    Connection, QueueHandle,
};

use super::{PointerInput, Surface};

/// niri 側の layer-rule から参照できるよう、namespace を固定する。
const LAYER_NAMESPACE: &str = "vmc-pet";

/// 他ウィンドウを押しのけないことを示す exclusive zone の値。
const NO_EXCLUSIVE_ZONE: i32 = -1;

const BYTES_PER_PIXEL: i32 = 4;

/// 常駐サーフェスの配置設定。
#[derive(Debug, Clone, Copy)]
pub struct LayerWindowConfig {
    pub width: u32,
    pub height: u32,
    /// 描画レート。場の更新レート(step_rate)とは意図的に分離する。
    pub render_fps: u32,
    /// 画面端からのマージン(論理ピクセル)。
    pub margin_right: i32,
    pub margin_bottom: i32,
}

impl Default for LayerWindowConfig {
    fn default() -> Self {
        Self {
            width: 384,
            height: 384,
            render_fps: 30,
            margin_right: 32,
            margin_bottom: 32,
        }
    }
}

/// Wayland 接続とサーフェスの状態を保持する。
/// 体のロジックは `body` の向こう側にあり、この構造体からは見えない。
pub struct LayerWindow {
    registry_state: RegistryState,
    output_state: OutputState,
    seat_state: SeatState,
    shm: Shm,

    compositor: CompositorState,
    layer: LayerSurface,
    pool: SlotPool,

    width: u32,
    height: u32,
    /// 1フレームに与える最小間隔。コンポジタのリフレッシュレートより粗く描く。
    frame_interval: Duration,
    last_draw: Instant,
    /// compositor から最初の configure を受け取るまで描画しない。
    configured: bool,
    should_exit: bool,
    /// 現在の入力領域。変化したときだけ再設定する。
    applied_input_region: Option<super::InputRegion>,

    pointer: Option<wl_pointer::WlPointer>,
    body: Box<dyn Surface>,
}

impl LayerWindow {
    /// Wayland に接続し、layer-shell サーフェスを作ってイベントループを回す。
    ///
    /// この関数はサーフェスが閉じられるまで戻らない。
    pub fn run(config: LayerWindowConfig, body: Box<dyn Surface>) -> Result<(), LayerWindowError> {
        let conn = Connection::connect_to_env().map_err(LayerWindowError::Connect)?;
        let (globals, mut event_queue) =
            registry_queue_init(&conn).map_err(LayerWindowError::Registry)?;
        let qh = event_queue.handle();

        let compositor = CompositorState::bind(&globals, &qh)
            .map_err(|e| LayerWindowError::MissingGlobal("wl_compositor", e))?;
        let layer_shell = LayerShell::bind(&globals, &qh)
            .map_err(|e| LayerWindowError::MissingGlobal("zwlr_layer_shell_v1", e))?;
        let shm =
            Shm::bind(&globals, &qh).map_err(|e| LayerWindowError::MissingGlobal("wl_shm", e))?;

        let surface = compositor.create_surface(&qh);
        let layer =
            layer_shell.create_layer_surface(&qh, surface, Layer::Top, Some(LAYER_NAMESPACE), None);
        layer.set_anchor(Anchor::BOTTOM | Anchor::RIGHT);
        layer.set_margin(0, config.margin_right, config.margin_bottom, 0);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer.set_size(config.width, config.height);
        layer.set_exclusive_zone(NO_EXCLUSIVE_ZONE);
        layer.commit();

        let pool_size = config.width as usize * config.height as usize * BYTES_PER_PIXEL as usize;
        let pool = SlotPool::new(pool_size, &shm).map_err(LayerWindowError::Pool)?;

        let mut window = LayerWindow {
            registry_state: RegistryState::new(&globals),
            output_state: OutputState::new(&globals, &qh),
            seat_state: SeatState::new(&globals, &qh),
            shm,
            compositor,
            layer,
            pool,
            width: config.width,
            height: config.height,
            frame_interval: frame_interval(config.render_fps),
            // 初回 configure での描画を待たせないよう、十分に過去の時刻から始める
            last_draw: Instant::now() - frame_interval(config.render_fps),
            configured: false,
            should_exit: false,
            applied_input_region: None,
            pointer: None,
            body,
        };

        while !window.should_exit {
            event_queue
                .blocking_dispatch(&mut window)
                .map_err(LayerWindowError::Dispatch)?;
        }
        Ok(())
    }

    /// 描画せずに次のフレームコールバックだけを要求する。
    fn request_frame(&self, qh: &QueueHandle<Self>) {
        let surface = self.layer.wl_surface();
        surface.frame(qh, surface.clone());
        surface.commit();
    }

    /// 体に1フレーム描かせ、次のフレームコールバックを要求する。
    fn draw(&mut self, qh: &QueueHandle<Self>) {
        self.last_draw = Instant::now();
        let stride = self.width as i32 * BYTES_PER_PIXEL;
        let (buffer, canvas) = match self.pool.create_buffer(
            self.width as i32,
            self.height as i32,
            stride,
            wl_shm::Format::Argb8888,
        ) {
            Ok(pair) => pair,
            Err(error) => {
                eprintln!("vmc-pet: failed to create shm buffer: {error}");
                return;
            }
        };

        self.body.draw(canvas, self.width, self.height);

        let surface = self.layer.wl_surface();
        surface.damage_buffer(0, 0, self.width as i32, self.height as i32);
        // 次フレームの通知を要求してから attach する(以降は frame コールバック駆動)
        surface.frame(qh, surface.clone());
        if let Err(error) = buffer.attach_to(surface) {
            eprintln!("vmc-pet: failed to attach buffer: {error}");
            return;
        }
        self.layer.commit();
    }

    /// クリックを受け取る矩形だけを入力領域に設定し、周囲は下のウィンドウへ透過させる。
    fn apply_input_region(&mut self, qh: &QueueHandle<Self>) {
        let region = self.body.input_region(self.width, self.height);
        if self.applied_input_region == Some(region) {
            return;
        }

        let wl_region = self.compositor.wl_compositor().create_region(qh, ());
        wl_region.add(region.x, region.y, region.width, region.height);
        self.layer.wl_surface().set_input_region(Some(&wl_region));
        wl_region.destroy();
        self.applied_input_region = Some(region);
    }

    /// ポインタ座標がこのサーフェス宛かを確かめてから体へ渡す。
    fn forward_pointer(&mut self, event: &PointerEvent) {
        if event.surface != *self.layer.wl_surface() {
            return;
        }
        let (x, y) = event.position;
        let input = match event.kind {
            PointerEventKind::Enter { .. } => PointerInput::Entered { x, y },
            PointerEventKind::Leave { .. } => PointerInput::Left,
            PointerEventKind::Motion { .. } => PointerInput::Moved { x, y },
            PointerEventKind::Press { .. } => PointerInput::Pressed { x, y },
            // リリースとスクロールは摂動の注入に使わないため無視する
            PointerEventKind::Release { .. } | PointerEventKind::Axis { .. } => return,
        };
        self.body.on_pointer(input);
    }
}

/// 描画機会(vblank)は離散的にしか訪れないため、間隔判定には許容幅を持たせる。
/// 許容幅がないと、60Hz のディスプレイで 30fps を狙ったときに閾値(33.33ms)と
/// vblank 2回分(33.34ms)がほぼ一致し、わずかなジッタで1フレーム落ちて実測 24fps まで下がる。
const FRAME_INTERVAL_TOLERANCE_DIVISOR: u32 = 8;

/// 前回の描画から `elapsed` 経過した時点で、今フレームを描くべきかを判定する。
fn should_draw(elapsed: Duration, frame_interval: Duration) -> bool {
    elapsed + frame_interval / FRAME_INTERVAL_TOLERANCE_DIVISOR >= frame_interval
}

/// 描画レートから1フレームあたりの最小間隔を求める。
/// 0 fps を指定された場合はコンポジタのリフレッシュレートに任せる。
fn frame_interval(render_fps: u32) -> Duration {
    if render_fps == 0 {
        return Duration::ZERO;
    }
    Duration::from_secs_f64(1.0 / render_fps as f64)
}

/// layer-shell サーフェスの生成に失敗する原因。
#[derive(Debug)]
pub enum LayerWindowError {
    Connect(wayland_client::ConnectError),
    Registry(wayland_client::globals::GlobalError),
    /// コンポジタが必要なグローバルを公開していない(layer-shell 非対応など)。
    MissingGlobal(&'static str, wayland_client::globals::BindError),
    Pool(smithay_client_toolkit::shm::CreatePoolError),
    Dispatch(wayland_client::DispatchError),
}

impl std::fmt::Display for LayerWindowError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Connect(e) => write!(f, "failed to connect to the wayland compositor: {e}"),
            Self::Registry(e) => write!(f, "failed to initialize the wayland registry: {e}"),
            Self::MissingGlobal(name, e) => {
                write!(f, "compositor does not provide {name}: {e}")
            }
            Self::Pool(e) => write!(f, "failed to create the shm pool: {e}"),
            Self::Dispatch(e) => write!(f, "wayland event dispatch failed: {e}"),
        }
    }
}

impl std::error::Error for LayerWindowError {}

impl LayerShellHandler for LayerWindow {
    fn closed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _layer: &LayerSurface) {
        self.should_exit = true;
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        _layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        // compositor が 0 を返した場合は自分の要求サイズを使う
        if configure.new_size.0 != 0 {
            self.width = configure.new_size.0;
        }
        if configure.new_size.1 != 0 {
            self.height = configure.new_size.1;
        }

        self.apply_input_region(qh);
        if !self.configured {
            self.configured = true;
        }
        self.draw(qh);
    }
}

impl CompositorHandler for LayerWindow {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _new_factor: i32,
    ) {
    }

    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _new_transform: wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _time: u32,
    ) {
        if !self.configured {
            return;
        }
        if !should_draw(self.last_draw.elapsed(), self.frame_interval) {
            // まだ描画時刻ではない。次のコールバックだけ要求して待つ
            self.request_frame(qh);
            return;
        }
        self.draw(qh);
    }

    fn surface_enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }
}

impl SeatHandler for LayerWindow {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _seat: wl_seat::WlSeat) {}

    fn new_capability(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability != Capability::Pointer || self.pointer.is_some() {
            return;
        }
        match self.seat_state.get_pointer(qh, &seat) {
            Ok(pointer) => self.pointer = Some(pointer),
            Err(error) => eprintln!("vmc-pet: failed to obtain the pointer: {error}"),
        }
    }

    fn remove_capability(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability != Capability::Pointer {
            return;
        }
        if let Some(pointer) = self.pointer.take() {
            pointer.release();
        }
    }

    fn remove_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _seat: wl_seat::WlSeat) {
    }
}

impl PointerHandler for LayerWindow {
    fn pointer_frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _pointer: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            self.forward_pointer(event);
        }
    }
}

impl OutputHandler for LayerWindow {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }

    fn update_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }

    fn output_destroyed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }
}

impl ShmHandler for LayerWindow {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for LayerWindow {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}

delegate_compositor!(LayerWindow);
delegate_output!(LayerWindow);
delegate_shm!(LayerWindow);
delegate_seat!(LayerWindow);
delegate_pointer!(LayerWindow);
delegate_layer!(LayerWindow);
delegate_registry!(LayerWindow);

// wl_region はイベントを持たないため、ディスパッチ先を用意するだけでよい
delegate_noop!(LayerWindow: ignore wl_region::WlRegion);

#[cfg(test)]
mod tests {
    use super::*;

    /// 60Hz のディスプレイで 30fps を狙う場合の1フレーム分の間隔。
    const VBLANK_60HZ: Duration = Duration::from_micros(16_667);

    #[test]
    fn should_draw_skips_a_single_vblank_but_accepts_two() {
        // Arrange: 30fps 目標 = 33.33ms 間隔
        let interval = frame_interval(30);

        // Act / Assert
        assert!(!should_draw(VBLANK_60HZ, interval), "1 vblank is too early");
        assert!(
            should_draw(VBLANK_60HZ * 2, interval),
            "2 vblanks must draw"
        );
    }

    #[test]
    fn should_draw_tolerates_jitter_around_the_threshold() {
        // Arrange: vblank 2回分が閾値をわずかに下回るケース
        let interval = frame_interval(30);
        let jittered = interval - Duration::from_micros(500);

        // Act
        let draws = should_draw(jittered, interval);

        // Assert: 許容幅がないとここで1フレーム落ちて実測 24fps になる
        assert!(draws, "a sub-millisecond shortfall must not drop the frame");
    }

    #[test]
    fn should_draw_always_draws_when_the_rate_is_unlimited() {
        // Arrange: render_fps 0 はコンポジタ任せを意味する
        let interval = frame_interval(0);

        // Act / Assert
        assert_eq!(interval, Duration::ZERO);
        assert!(should_draw(Duration::ZERO, interval));
    }
}
