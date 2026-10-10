//! 姿勢をドットグリッドへ写す投影。
//!
//! ドットグリッドでは CSS の `transform: rotate3d()` が使えないので、姿勢は
//! **オルソ投影の「厚みのある板(スラブ)」**として描く。体は厚み
//! `SLAB_THICKNESS_CELLS` セルの板で、描くのは2つだけ。
//!
//! - **前面**(板の手前の面)… 見えているセルを1回だけ描く
//! - **側面**(板の縁の帯)… シルエットの**境界にあるセル**だけ、厚みのぶんだけ点を掃く
//!
//! pitch(前後の傾き)を付けると、上端の側面が**輪郭の外側へ帯として現れる**
//! (帯の高さ = 厚み × sin(角度))。yaw なら左右の縁に同じ帯が出る。roll(画面内の傾き)は
//! 板ごと回るだけで帯は出ないが、小さい角度でも読める(spike の実測)。
//!
//! これで spike の「**別の面が見える**という離散的な変化」をそのまま作れる。
//! CSS の `perspective` が担っていた役割を、セル単位の厚みが担う。
//!
//! **層を何枚も重ねる作りは採らなかった。** 板を「見えているセルの層」として何枚も
//! 重ねると、ずれた半透明の像が重なって「雲」になり、spike が実測で否定した混合の
//! 失敗(不透明度の重ね合わせ)を描画の中で再現してしまう。見えるのは常に前面と
//! 側面だけで、板の奥は隠れる。
//!
//! オルソ投影(透視除算なし)にした理由は2つある。
//!
//! - **逆写像が解ける。** 描いたドットと、クリックした位置が食い違わない
//!   (`DotGrid::cell_at_posed`)
//! - **つまみが増えない。** CSS では `perspective` という距離のパラメータが要ったが、
//!   ここでは厚みだけで足りる。同じ効果を2つの量で作ると、しきい値の実測が濁る
//!
//! 姿勢が 0 のときは側面の掃きが1点に潰れるので、**姿勢を足しても見た目は
//! 1ビットも変わらない。**

use core::f32::consts::PI;
use vmc_pet_body::gesture::{ResolvedPose, SLAB_THICKNESS_CELLS};

const DEGREES_TO_RADIANS: f32 = PI / 180.0;

/// 側面を何個の点で掃くか。厚み 3.5 セルを 4 分割するので点の間隔は 0.875 セル
/// (ドットの直径より狭い)になり、帯が途切れて見えない。
pub const WALL_STEPS: usize = 5;

/// 側面の高さがこれ未満(セル)なら描かない。見えない帯のために点を打たない。
/// **この分岐が、姿勢ゼロで見た目が変わらないことを保証している。**
pub const WALL_MERGE_CELLS: f32 = 0.25;

/// 回転行列(行優先)の積。
fn multiply(a: [[f32; 3]; 3], b: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let mut out = [[0.0f32; 3]; 3];
    for row in 0..3 {
        for column in 0..3 {
            out[row][column] = (0..3).map(|k| a[row][k] * b[k][column]).sum();
        }
    }
    out
}

/// x 軸まわりの回転(pitch)。
fn rotate_x(radians: f32) -> [[f32; 3]; 3] {
    let (sin, cos) = (radians.sin(), radians.cos());
    [[1.0, 0.0, 0.0], [0.0, cos, -sin], [0.0, sin, cos]]
}

/// y 軸まわりの回転(yaw)。
fn rotate_y(radians: f32) -> [[f32; 3]; 3] {
    let (sin, cos) = (radians.sin(), radians.cos());
    [[cos, 0.0, sin], [0.0, 1.0, 0.0], [-sin, 0.0, cos]]
}

/// z 軸まわりの回転(roll)。
fn rotate_z(radians: f32) -> [[f32; 3]; 3] {
    let (sin, cos) = (radians.sin(), radians.cos());
    [[cos, -sin, 0.0], [sin, cos, 0.0], [0.0, 0.0, 1.0]]
}

/// 姿勢を、グリッド中心を原点とするセル単位の座標系へ写す投影。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PoseProjection {
    /// 回転行列。局所座標(セル単位)をカメラ座標へ。
    matrix: [[f32; 3]; 3],
    /// 画面内の平行移動(セル単位、y は下向きが正)。
    offset: (f32, f32),
    /// 体の大きさ。
    scale: f32,
}

impl Default for PoseProjection {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl PoseProjection {
    /// 姿勢を掛けない投影。
    pub const IDENTITY: PoseProjection = PoseProjection {
        matrix: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        offset: (0.0, 0.0),
        scale: 1.0,
    };

    /// 姿勢から作る。回転の順は yaw → pitch → roll(大きい動きから先に当てる)。
    pub fn from_pose(pose: &ResolvedPose) -> Self {
        let radians = |degrees: f32| degrees * DEGREES_TO_RADIANS;
        let matrix = multiply(
            rotate_z(radians(pose.roll_deg)),
            multiply(
                rotate_x(radians(pose.pitch_deg)),
                rotate_y(radians(pose.yaw_deg)),
            ),
        );
        Self {
            matrix,
            offset: pose.offset_cells,
            scale: pose.scale,
        }
    }

    /// 体の大きさ(`size`)。ドットの半径にも同じ倍率を掛ける。
    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// 局所座標(グリッド中心からのセル単位、z は板の厚み方向)を、
    /// 画面のセル単位(グリッド中心基準、y は下向き)へ。
    pub fn project(&self, local: (f32, f32, f32)) -> (f32, f32) {
        let row = |index: usize| {
            self.matrix[index][0] * local.0
                + self.matrix[index][1] * local.1
                + self.matrix[index][2] * local.2
        };
        (
            row(0) * self.scale + self.offset.0,
            row(1) * self.scale + self.offset.1,
        )
    }

    /// 画面のセル単位(グリッド中心基準)から、板の中央面(z = 0)上の局所座標へ。
    ///
    /// 板と視線が平行に近い(行列の左上 2x2 が潰れている)ときは `None`。
    /// 見えているドットと押した位置を一致させるために使う(`DotGrid::cell_at_posed`)。
    pub fn unproject(&self, screen: (f32, f32)) -> Option<(f32, f32)> {
        let x = (screen.0 - self.offset.0) / self.scale;
        let y = (screen.1 - self.offset.1) / self.scale;
        let (a, b) = (self.matrix[0][0], self.matrix[0][1]);
        let (c, d) = (self.matrix[1][0], self.matrix[1][1]);
        let determinant = a * d - b * c;
        if determinant.abs() < 1e-4 {
            return None;
        }
        Some(((x * d - y * b) / determinant, (a * y - c * x) / determinant))
    }

    /// 側面が画面に落とす帯の高さ(セル単位)。板の法線が画面内で持つ長さそのもので、
    /// pitch/yaw が無ければ 0。
    pub fn wall_height_cells(&self) -> f32 {
        let (x, y) = (self.matrix[0][2], self.matrix[1][2]);
        SLAB_THICKNESS_CELLS * (x * x + y * y).sqrt()
    }

    ///側面を描くか。潰れて見えない高さなら描かない(姿勢ゼロとの同一性もこれで守る)。
    pub fn shows_walls(&self) -> bool {
        self.wall_height_cells() >= WALL_MERGE_CELLS
    }

    /// 前面(板の手前の面)の z(セル単位)。
    pub fn front_z(&self) -> f32 {
        -SLAB_THICKNESS_CELLS / 2.0
    }

    /// 側面を掃く `step` 番目(0..`WALL_STEPS`)の z(セル単位)。0 が前面。
    pub fn wall_z(&self, step: usize) -> f32 {
        let step = step.min(WALL_STEPS - 1);
        -SLAB_THICKNESS_CELLS / 2.0
            + SLAB_THICKNESS_CELLS * step as f32 / (WALL_STEPS - 1) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vmc_pet_body::gesture::ResolvedPose;

    fn posed(pitch: f32, yaw: f32, roll: f32) -> PoseProjection {
        PoseProjection::from_pose(&ResolvedPose {
            pitch_deg: pitch,
            yaw_deg: yaw,
            roll_deg: roll,
            ..ResolvedPose::IDENTITY
        })
    }

    /// セル単位の `(u, v)` を、投影の逆写像で戻す。
    fn round_trip(projection: PoseProjection, u: f32, v: f32) -> (f32, f32) {
        let screen = projection.project((u, v, 0.0));
        projection.unproject(screen).expect("the face must be visible")
    }

    #[test]
    fn the_identity_projection_leaves_the_lattice_alone() {
        // Arrange / Act / Assert
        let projection = PoseProjection::IDENTITY;
        assert_eq!(projection.project((5.0, -3.0, 0.0)), (5.0, -3.0));
        assert_eq!(projection.project((5.0, -3.0, 1.75)), (5.0, -3.0));
        assert_eq!(
            PoseProjection::from_pose(&ResolvedPose::IDENTITY),
            PoseProjection::IDENTITY
        );
    }

    #[test]
    fn a_flat_pose_hides_the_walls_entirely() {
        // Arrange: 姿勢ゼロ
        let projection = PoseProjection::from_pose(&ResolvedPose::IDENTITY);

        // Act / Assert: 側面の高さが 0 なので描かない = 元の1枚と区別が付かない
        assert_eq!(projection.wall_height_cells(), 0.0);
        assert!(!projection.shows_walls());
    }

    #[test]
    fn roll_alone_never_shows_a_wall() {
        // Arrange: 画面内の傾きだけを与える
        let projection = posed(0.0, 0.0, 40.0);

        // Act / Assert: roll は板の法線を動かさないので、側面は出ない
        assert!(!projection.shows_walls());
        // それでも格子は回っている(局所点が動く)
        let (x, y) = projection.project((10.0, 0.0, 0.0));
        assert!(x < 10.0 && y > 0.0, "got ({x}, {y})");
    }

    #[test]
    fn pitch_raises_a_wall_of_the_thickness_times_the_sine() {
        // Arrange: 帯が読める角度
        let projection = posed(30.0, 0.0, 0.0);

        // Act
        let height = projection.wall_height_cells();

        // Assert: 3.5 セル × sin30度 = 1.75 セル
        assert!((height - 1.75).abs() < 1e-4, "got {height}");
    }

    #[test]
    fn the_walls_appear_at_the_thickness_threshold_angle() {
        // Arrange: 帯の高さが WALL_MERGE_CELLS を超える角度
        let threshold = (WALL_MERGE_CELLS / SLAB_THICKNESS_CELLS).asin().to_degrees();

        // Act / Assert: その手前では描かない
        assert!(!posed(threshold - 1.0, 0.0, 0.0).shows_walls());
        assert!(posed(threshold + 1.0, 0.0, 0.0).shows_walls());

        // ドット1個ぶん(1セル)の帯になる角度は 16.6度のはず(実測で確かめる値)
        let one_dot = (1.0 / SLAB_THICKNESS_CELLS).asin().to_degrees();
        assert!(
            (one_dot - 16.6).abs() < 0.2,
            "ドット1個ぶんの帯は 16.6度のはず, got {one_dot}"
        );
    }

    #[test]
    fn pitch_lifts_the_wall_above_the_front_edge() {
        // Arrange: のけぞる(正の pitch)
        let projection = posed(30.0, 0.0, 0.0);

        // Act: 上端のセル(v = -10)の前面と、側面の一番奥
        let front = projection.project((0.0, -10.0, projection.front_z()));
        let wall_back = projection.project((0.0, -10.0, projection.wall_z(WALL_STEPS - 1)));

        // Assert: 側面は前面より上へ伸びる(帯が輪郭の外に出る)
        assert!(wall_back.1 < front.1, "front={front:?} wall={wall_back:?}");
        assert!(
            (front.1 - wall_back.1 - 1.75).abs() < 1e-4,
            "got {}",
            front.1 - wall_back.1
        );
    }

    #[test]
    fn yaw_shows_walls_at_the_sides() {
        // Arrange
        let projection = posed(0.0, 45.0, 0.0);

        // Act
        let front = projection.project((0.0, 0.0, projection.front_z()));
        let wall_back = projection.project((0.0, 0.0, projection.wall_z(WALL_STEPS - 1)));

        // Assert: 横に分かれる(縦には分かれない)
        assert!((front.1 - wall_back.1).abs() < 1e-4);
        assert!(
            (front.0 - wall_back.0).abs() > 2.0,
            "got {front:?} {wall_back:?}"
        );
    }

    #[test]
    fn the_offset_shifts_the_whole_grid_in_screen_cells() {
        // Arrange: 沈む姿勢(下向きが正)
        let projection = PoseProjection::from_pose(&ResolvedPose {
            offset_cells: (0.0, 1.25),
            ..ResolvedPose::IDENTITY
        });

        // Act / Assert
        assert_eq!(projection.project((0.0, 0.0, 0.0)), (0.0, 1.25));
        assert_eq!(projection.unproject((0.0, 2.25)), Some((0.0, 1.0)));
    }

    #[test]
    fn the_scale_stretches_the_lattice_about_the_centre() {
        // Arrange
        let projection = PoseProjection::from_pose(&ResolvedPose {
            scale: 1.25,
            ..ResolvedPose::IDENTITY
        });

        // Act / Assert: 中心は動かず、外側だけが広がる
        assert_eq!(projection.project((0.0, 0.0, 0.0)), (0.0, 0.0));
        assert_eq!(projection.project((4.0, 0.0, 0.0)), (5.0, 0.0));
    }

    #[test]
    fn unprojecting_and_projecting_agree_for_every_pose_in_the_vocabulary() {
        // Arrange: 語彙の姿勢をひととおり回す
        for pitch in [-30.0f32, 0.0, 25.0, 70.0] {
            for yaw in [-45.0f32, 0.0, 45.0] {
                for roll in [-30.0f32, 0.0, 30.0] {
                    let projection = posed(pitch, yaw, roll);

                    // Act / Assert: 格子の上の点は、投影して戻すと元に戻る
                    for (u, v) in [(0.0, 0.0), (7.5, -3.5), (-11.0, 9.0)] {
                        let (back_u, back_v) = round_trip(projection, u, v);
                        assert!(
                            (back_u - u).abs() < 1e-3 && (back_v - v).abs() < 1e-3,
                            "pitch={pitch} yaw={yaw} roll={roll} ({u}, {v}) -> ({back_u}, {back_v})"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_pose_that_flattens_the_face_cannot_be_unprojected() {
        // Arrange: 板を視線と平行にする(90度の yaw)- 実際には語彙に無いが、破綻しないこと
        let projection = posed(0.0, 90.0, 0.0);

        // Act / Assert
        assert_eq!(projection.unproject((0.0, 0.0)), None);
    }
}
