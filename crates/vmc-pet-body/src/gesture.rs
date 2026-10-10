//! 姿勢(ジェスチャー)の語彙と、ペットの内部状態 → 姿勢の写像。
//!
//! 経緯: `docs/experiments/css-gesture.md`(moon で完走した spike)。決定モデルの出力を
//! CSS アニメーションのジェスチャーに写し、「人が読める動きの差」が出るかを確かめた。
//! ここは、そこで確定した設計則を、CSS を使えない PC 版(wlr-layer-shell の常駐
//! ウィンドウにドットグリッド)へ持ち込む側。段階1 では駆動源を決定モデルではなく
//! **ペット自身の内部状態**(エネルギー・まとまり・テンポ・放置時間)にする。
//! 決定モデルを入力にする段階2 は、この写像(`weights`)を分布に差し替えるだけで済む。
//!
//! 持ち込んだ設計則(すべて css-gesture.md の実測に基づく):
//!
//! - 語彙は**姿勢パラメータ空間の点**としてデータで定義する(`GESTURES`)
//! - 混合は**重み付き平均(補間)**。複数を重ねて見せると「半透明の図形がずれた雲」になり
//!   人が読めない(実測)。1体だけを描く
//! - **符号付きの次元は混合で相殺する**(`rise` は8状態平均で55%が消えていた)。
//!   上下は非負の `lift`/`sink` に割り、捨てられる量 `2*min(lift, sink)` を roll に戻す
//! - **量ではなく運動の種類を変える。** 振幅差(1.31x)は人に読めない。回転は読める
//! - **同じチャネルに「姿勢」と「振れ」を載せると奪い合う**(`rot_x` 40度は姿勢差を洗い流す。
//!   0.45倍に絞ると「上面が見える時間の差」が 16pt → 39pt になった)
//! - 読みやすさのしきい値は `perspective: 170px` で pitch 15度 / roll 数度。ただし
//!   これは CSS の実測なので、ドットグリッドでは**自分の描画で測り直す**
//!   (`src/render/pose.rs` と `docs/experiments/expression-layer.md`)
//!
//! 角度(度)は CSS と単位が同じなのでそのまま持ち込める。持ち込み直しが要るのは
//! 長さだけで、pose unit → セルの換算(`CELLS_PER_POSE_UNIT`)を新しい定数にしてある。

use crate::math::{rem_euclidf, sinf, sqrtf};
use core::f32::consts::PI;

/// pose unit 1.0 を、ドットグリッドの何セルとして描くか。
///
/// spike は 118px の枠に対して `px_per_unit = 18.0`(枠の約 0.15 倍)だった。
/// 表示は32×32セルで、体はおよそ 20×20 セル。同じ比率なら 3 セル前後だが、
/// 上下の移動は「量の差は読めない」と実測済みで、大きさそのものは情報を持たない
/// (読ませたいのは回転と lift/sink の符号)。**揺れが体の輪郭を越えない**ことを優先し、
/// 小さめの 2.5 セルにする。体の高さ 20 セルに対して最大 0.6 unit = 1.5 セル。
pub const CELLS_PER_POSE_UNIT: f32 = 2.5;

/// 混合の「葛藤」 `2*min(lift, sink)` 1.0 を、roll の振れ幅なん度にするか。
///
/// spike の実測値(60.0)をそのまま持ち込む。roll は画面内の傾きなので
/// 小さい角度でも読め、`nag` で 25.7度 / `fixed_bug` で 14.8度 / `approved` で 5.2度に
/// 分離した値。角度の単位は CSS と同じなので換算は要らない。
pub const DEG_PER_CONFLICT: f32 = 60.0;

/// 板の厚み(セル)。pitch/yaw を付けたとき、**側面が画面に落とす帯の高さ** =
/// この値 × sin(角度)。
///
/// 3.5 セルにすると、帯の高さがドット1個ぶん(表示で約12px)になる角度が 16.6度、
/// 2個ぶんになる角度が 34.8度になる。spike が `perspective: 170px` で測った
/// 「15度で上面が見え始める / 30度で立体として明確」とほぼ同じ値。
/// これは同じ条件(読める下限 = ドット1個ぶんの分離)を別の手段で満たしているため。
/// **実測での確認は `docs/experiments/expression-layer.md` を参照**(予測と実測を分けて書いてある)。
pub const SLAB_THICKNESS_CELLS: f32 = 3.5;

/// 明滅(`flick`)の速さ。位相の何倍で回すか。
const BLINK_TURNS: f32 = 3.0;

/// 体の大きさ(`size`)として許す範囲。混合で薄まった値でも破綻しないように丸める。
const MIN_POSE_SCALE: f32 = 0.6;
const MAX_POSE_SCALE: f32 = 1.3;

/// 放置時間を「最近かまってもらえた」とみなす長さ(秒)。これを過ぎると 0 になる。
///
/// エネルギーが尽きる速さ(150秒)より短くしてある。同じ長さにすると
/// 「元気だけれど長く放置されている」状態が作れなくなり、写像の2つの入力が
/// 同じものになってしまう。
pub const RECENT_CARE_SECONDS: f32 = 90.0;

/// 「まとまり」を測る半径(セル)。重心からこの距離の中にある総量の割合を
/// `FieldView::cohesion` が返す。
///
/// **実測で決めた値**(`docs/experiments/expression-layer.md` の「まとまりを測る半径」)。
/// 出荷している4種 + 粒子の体で測ると、r=6 では正常な Orbium でも 0.61 になり
/// (体そのものの半径より小さいので、正常な形が「崩れている」と読まれる)、
/// r=10 なら健康な体が 0.92〜1.00 に揃う。**体の大きさに合わせて 10 セル**を採る。
/// PC 版と M5Stack 版で同じ値を使うためここに置く。
pub const COHESION_RADIUS_CELLS: f32 = 10.0;

/// 姿勢パラメータ空間の1点。ジェスチャー語彙はこの空間の点として定義する。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pose {
    /// 静的な pitch(度)。正 = のけぞる(板の上面が見える)/ 負 = うつむく。
    /// `rot_x` は「振れ幅」で、こちらは「姿勢」。同じチャネルを奪い合うので分けてある。
    pub tilt_x: f32,
    /// 上下のボブの振幅(pose unit)。平行移動の揺れはこれだけ残した。
    pub bob: f32,
    /// pitch(前後の回転)の振れ幅(度)。`nod` / `recoil` / `perk_up` が使う。
    pub rot_x: f32,
    /// yaw(左右を向く回転)の振れ幅(度)。`shake` が使う。
    pub rot_y: f32,
    /// roll(画面内の傾き)の振れ幅(度)。`idle` / `flicker` が使い、
    /// 混合の「葛藤」もここに加算される。
    pub rot_z: f32,
    /// 浮き上がる姿勢の強さ(pose unit、0 以上)。`recoil` / `perk_up` / `lean_in` が立てる。
    pub lift: f32,
    /// 沈む姿勢の強さ(pose unit、0 以上)。`settle` / `nod` が立てる。
    pub sink: f32,
    /// 体の大きさ。1.0 が基準。
    pub size: f32,
    /// 動きの速さ(周期の逆数の相対値。大きいほど速い)。
    pub freq: f32,
    /// 明滅の強さ(0-1)。0 なら明滅しない。
    pub flick: f32,
}

impl Pose {
    /// 何も主張しない姿勢。姿勢層を切ったときの値でもある。
    pub const IDENTITY: Pose = Pose {
        tilt_x: 0.0,
        bob: 0.0,
        rot_x: 0.0,
        rot_y: 0.0,
        rot_z: 0.0,
        lift: 0.0,
        sink: 0.0,
        size: 1.0,
        freq: 1.0,
        flick: 0.0,
    };

    /// 8ジェスチャーの重み付き平均で1点に畳む。重みは合計 1 であることを前提にする。
    fn scaled(self, weight: f32) -> Pose {
        Pose {
            tilt_x: self.tilt_x * weight,
            bob: self.bob * weight,
            rot_x: self.rot_x * weight,
            rot_y: self.rot_y * weight,
            rot_z: self.rot_z * weight,
            lift: self.lift * weight,
            sink: self.sink * weight,
            // 大きさと速さは「量」ではなく比率なので、加重平均で薄まると
            // 1.0 に張り付いて動きが消える。差(1.0 からのずれ)を平均して戻す。
            size: (self.size - 1.0) * weight,
            freq: (self.freq - 1.0) * weight,
            flick: self.flick * weight,
        }
    }

    fn accumulate(&mut self, other: Pose) {
        self.tilt_x += other.tilt_x;
        self.bob += other.bob;
        self.rot_x += other.rot_x;
        self.rot_y += other.rot_y;
        self.rot_z += other.rot_z;
        self.lift += other.lift;
        self.sink += other.sink;
        self.size += other.size;
        self.freq += other.freq;
        self.flick += other.flick;
    }

    /// 8ジェスチャーの重み付き平均(補間)。合計の重みは 1 でなくてよい。
    ///
    /// `size` と `freq` は比率なので、1.0 からのずれを平均して戻す。
    /// 何も選ばれていない(合計が 0)ときは `IDENTITY`。
    pub fn mix(gestures: &[Gesture; GESTURE_COUNT], weights: &[f32; GESTURE_COUNT]) -> Pose {
        let total: f32 = weights.iter().sum();
        if total <= 0.0 {
            return Pose::IDENTITY;
        }
        let mut sum = Pose {
            size: 0.0,
            freq: 0.0,
            ..Pose::IDENTITY
        };
        for (gesture, weight) in gestures.iter().zip(weights.iter()) {
            sum.accumulate(gesture.pose.scaled(*weight));
        }
        Pose {
            tilt_x: sum.tilt_x / total,
            bob: sum.bob / total,
            rot_x: sum.rot_x / total,
            rot_y: sum.rot_y / total,
            rot_z: sum.rot_z / total,
            lift: sum.lift / total,
            sink: sum.sink / total,
            size: 1.0 + sum.size / total,
            freq: 1.0 + sum.freq / total,
            flick: sum.flick / total,
        }
    }
}

/// ジェスチャー1つ = 名前 + 姿勢空間の点。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gesture {
    pub name: &'static str,
    pub pose: Pose,
}

impl Gesture {
    const fn new(name: &'static str, pose: Pose) -> Self {
        Self { name, pose }
    }
}

/// 語彙は8個。並び順は `weights` の戻り値の添字になる。
pub const GESTURE_COUNT: usize = 8;

pub const GESTURES: [Gesture; GESTURE_COUNT] = [
    Gesture::new(
        "idle",
        Pose {
            tilt_x: 0.0,
            bob: 0.22,
            rot_x: 3.0,
            rot_y: 0.0,
            rot_z: 5.0,
            lift: 0.0,
            sink: 0.0,
            size: 1.0,
            freq: 1.0,
            flick: 0.0,
        },
    ),
    Gesture::new(
        "perk_up",
        Pose {
            // tilt_x 70度 は spike の実測で決めた「のけぞって上面が見える」姿勢。
            // 読みやすさのしきい値(15度)を確実に超えさせるため。
            tilt_x: 70.0,
            bob: 0.55,
            rot_x: 13.0,
            rot_y: 0.0,
            rot_z: 0.0,
            lift: 0.55,
            sink: 0.0,
            size: 1.1,
            freq: 1.7,
            flick: 0.0,
        },
    ),
    Gesture::new(
        "nod",
        Pose {
            // tilt_x は 0。負の tilt を与えると perk_up と相殺して 10.6度が消える
            // (spike が計算で見つけた罠)。nod は振れであって傾きではない。
            tilt_x: 0.0,
            bob: 0.18,
            rot_x: 18.0,
            rot_y: 0.0,
            rot_z: 0.0,
            lift: 0.0,
            sink: 0.15,
            size: 0.96,
            freq: 1.45,
            flick: 0.0,
        },
    ),
    Gesture::new(
        "shake",
        Pose {
            tilt_x: 0.0,
            bob: 0.0,
            rot_x: 0.0,
            // rot_y(yaw)は姿勢差と干渉しない別チャネルなので、絞らずに残す。
            rot_y: 45.0,
            rot_z: 0.0,
            lift: 0.0,
            sink: 0.0,
            size: 1.0,
            freq: 1.65,
            flick: 0.0,
        },
    ),
    Gesture::new(
        "lean_in",
        Pose {
            tilt_x: -18.0,
            bob: 0.28,
            rot_x: 5.0,
            rot_y: 0.0,
            rot_z: 0.0,
            lift: 0.45,
            sink: 0.0,
            size: 1.24,
            freq: 0.75,
            flick: 0.0,
        },
    ),
    Gesture::new(
        "recoil",
        Pose {
            tilt_x: 26.0,
            bob: 0.5,
            // rot_x は spike の実測どおり 0.45 倍に絞った値(40 → 18度)。
            // 振れが大きいと静的な姿勢差が洗い流される。
            rot_x: 18.0,
            rot_y: 0.0,
            rot_z: 0.0,
            lift: 0.95,
            sink: 0.0,
            size: 0.82,
            freq: 2.2,
            flick: 0.0,
        },
    ),
    Gesture::new(
        "settle",
        Pose {
            tilt_x: -24.0,
            bob: 0.16,
            rot_x: 5.0,
            rot_y: 0.0,
            rot_z: 0.0,
            lift: 0.0,
            sink: 1.05,
            size: 0.7,
            freq: 0.55,
            flick: 0.0,
        },
    ),
    Gesture::new(
        "flicker",
        Pose {
            tilt_x: 0.0,
            bob: 0.04,
            rot_x: 1.0,
            rot_y: 0.0,
            rot_z: 8.0,
            lift: 0.0,
            sink: 0.0,
            size: 1.0,
            freq: 1.1,
            flick: 1.0,
        },
    ),
];

/// 名前から添字を引く(`--pose <名前>` の解釈に使う)。
pub fn gesture_index(name: &str) -> Option<usize> {
    GESTURES.iter().position(|gesture| gesture.name == name)
}

/// 重みが最大のジェスチャーの添字。同点なら先に並んでいる方。
pub fn dominant(weights: &[f32; GESTURE_COUNT]) -> usize {
    let mut best = 0;
    for index in 1..GESTURE_COUNT {
        if weights[index] > weights[best] {
            best = index;
        }
    }
    best
}

/// 姿勢を決める入力。すべて体の外側の少数の状態変数から来る。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ExpressionInputs {
    /// エネルギー(0.0..=1.0)。`Vitality` の値。
    pub energy: f32,
    /// 体のまとまり(0.0..=1.0)。1 が塊、0 がばらばら。
    pub cohesion: f32,
    /// 体の時間の進み方(1.0 がいつもどおり)。`Mood::tempo()` の値。
    pub tempo: f32,
    /// 最後にかまってもらってからの秒数。
    pub unattended_seconds: f32,
}

/// がっかりしきったときのテンポ(`mood::DISAPPOINTED_TEMPO` と同じ値)。
/// `mood` 側の定数は private なので、ここでは写像の入力としての下限を持つ。
const SLOWEST_TEMPO: f32 = 0.6;

/// 状態 → 8ジェスチャーの重み(合計 1)。
///
/// **駆動源ごとに項を1つ**にしてある。どの状態がどの項を立てたかが1行で言える方が、
/// 段階2 で決定モデルの分布に差し替えるときに、比べる相手として役に立つ。
/// 各項の根拠は `docs/experiments/expression-layer.md`。
pub fn weights(inputs: &ExpressionInputs) -> [f32; GESTURE_COUNT] {
    let energy = inputs.energy.clamp(0.0, 1.0);
    let cohesion = inputs.cohesion.clamp(0.0, 1.0);
    // がっかりの度合い: テンポ 1.0 で 0、0.6 で 1
    let slow = ((1.0 - inputs.tempo) / (1.0 - SLOWEST_TEMPO)).clamp(0.0, 1.0);
    // 最近かまってもらえた度合い: 90秒で 0
    let fresh = 1.0 - (inputs.unattended_seconds / RECENT_CARE_SECONDS).clamp(0.0, 1.0);
    // 元気の「中間」でだけ 1 になる山(迷いの素)
    let uncertainty = 4.0 * energy * (1.0 - energy);

    let mut weights = [
        0.45,                      // idle: 基準。何も主張しない
        1.10 * energy * fresh,     // perk_up: 元気で、最近かまってもらえた
        0.95 * energy * cohesion,  // nod: 元気で、体がまとまっている
        1.15 * (1.0 - cohesion),   // shake: 体がまとまらない
        0.90 * fresh * (1.0 - cohesion), // lean_in: 最近かまってもらい、まだ整っていない
        1.20 * (1.0 - energy) * slow, // recoil: 弱っていて、がっかりもしている
        1.05 * (1.0 - energy),     // settle: 弱っている
        // flicker: 元気が中間で、まとまっていない。
        // 係数だけ 1.6 と大きいのは、この項が `shake` と同時に立つため
        // (どちらも「まとまっていない」が、energy が中間のときは迷いの方が主役になる)
        1.60 * uncertainty * (1.0 - cohesion),
    ];
    let total: f32 = weights.iter().sum();
    if total > 0.0 {
        for weight in weights.iter_mut() {
            *weight /= total;
        }
    }
    weights
}

/// 位相を固定したときの姿勢(実際に描く値)。
///
/// 静的な姿勢(`tilt_x`・`lift - sink`)と、振れ(`sin(位相)` に比例する部分)を
/// 1つに畳んだもの。描画側はこれだけを見る。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResolvedPose {
    /// 前後の傾き(度)。正 = のけぞって上面が見える。
    pub pitch_deg: f32,
    /// 左右の傾き(度)。
    pub yaw_deg: f32,
    /// 画面内の傾き(度)。
    pub roll_deg: f32,
    /// 画面内の平行移動(セル単位、y は下向きが正)。
    pub offset_cells: (f32, f32),
    /// 体の大きさ。
    pub scale: f32,
    /// 不透明度に掛ける明滅(0.0..=1.0)。1.0 なら明滅しない。
    pub visibility: f32,
}

impl ResolvedPose {
    /// 姿勢を掛けない状態。
    pub const IDENTITY: ResolvedPose = ResolvedPose {
        pitch_deg: 0.0,
        yaw_deg: 0.0,
        roll_deg: 0.0,
        offset_cells: (0.0, 0.0),
        scale: 1.0,
        visibility: 1.0,
    };

    /// 板の面から立ち上がっている角度の大きさ(度)。0 なら層が重なって見えない。
    pub fn tilt_from_the_face(&self) -> f32 {
        sqrtf(self.pitch_deg * self.pitch_deg + self.yaw_deg * self.yaw_deg)
    }
}

/// 明滅(`flick`)で、ドットがどこまで薄くなるか(0.0 = 消える、1.0 = 薄くならない)。
///
/// 1.0 にすると `flick = 1` の `flicker` 単独で不透明度が 0 になり、**消えている
/// 瞬間ができる**。ペットの体が完全に消えるのは「迷い」ではなく「不在」に見えるので、
/// 薄れるが消えない 0.7 にする(最小の見え方 = 元の 0.3 倍)。
const FLICK_DEPTH: f32 = 0.7;

/// 姿勢空間の点・葛藤・位相から、実際に描く値を決める。
///
/// - 静的な pitch は `tilt_x`。振れは `sin(位相)` に比例する
/// - roll は `rot_z` に**葛藤 `2*min(lift, sink)` × `DEG_PER_CONFLICT`** を足した振幅。
///   混合で捨てられる量そのものを、縦振幅ではなく画面内の傾きとして回復する
///   (roll は小さい角度でも読める。spike の実測)
/// - 上下は `sink - lift`(静的な沈み/浮き)+ `bob`(揺れ)
pub fn resolve(pose: &Pose, conflict: f32, phase: f32) -> ResolvedPose {
    let wave = sinf(phase);
    let roll_amplitude = pose.rot_z + DEG_PER_CONFLICT * conflict;
    let blink = 0.5 + 0.5 * sinf(phase * BLINK_TURNS);
    ResolvedPose {
        pitch_deg: pose.tilt_x + pose.rot_x * wave,
        yaw_deg: pose.rot_y * wave,
        roll_deg: roll_amplitude * wave,
        offset_cells: (
            0.0,
            ((pose.sink - pose.lift) + pose.bob * wave) * CELLS_PER_POSE_UNIT,
        ),
        scale: pose.size.clamp(MIN_POSE_SCALE, MAX_POSE_SCALE),
        visibility: 1.0 - pose.flick.clamp(0.0, 1.0) * FLICK_DEPTH * (1.0 - blink),
    }
}

/// 混合の「葛藤」: 符号付きの平均なら捨てられていた量 `2*min(lift, sink)`。
pub fn conflict_of(pose: &Pose) -> f32 {
    2.0 * pose.lift.min(pose.sink)
}

/// いまの重み・混合・位相をまとめて持つ、姿勢層の本体。
///
/// `Pet` は1つ持ち、描画のたびに `tick` する。位相を明示的に固定できるのは、
/// **動いている絵では差が読めない**ことを spike が実測しているため
/// (アニメを 0% に凍結して測る必要があった)。検証は `set_phase` で止めて行う。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Expression {
    phase: f32,
    unattended_seconds: f32,
    weights: [f32; GESTURE_COUNT],
    pose: Pose,
    conflict: f32,
    /// 1つのジェスチャーに固定されているか(`--pose <名前>` と、しきい値の実測に使う)。
    pinned: Option<usize>,
}

impl Default for Expression {
    fn default() -> Self {
        Self::new()
    }
}

impl Expression {
    /// 位相 0・最後にかまってもらった直後から始める。
    pub fn new() -> Self {
        Self {
            phase: 0.0,
            unattended_seconds: 0.0,
            weights: weights(&ExpressionInputs {
                energy: 1.0,
                cohesion: 1.0,
                tempo: 1.0,
                unattended_seconds: 0.0,
            }),
            pose: Pose::IDENTITY,
            conflict: 0.0,
            pinned: None,
        }
    }

    /// 世話が来た(クリックされた)ことを知らせる。放置時間が 0 に戻る。
    pub fn record_care(&mut self) {
        self.unattended_seconds = 0.0;
    }

    /// `seconds` ぶん時間を進め、そのときの姿勢を返す。
    ///
    /// 位相の進み方は混合した `freq` で決まる(`Expression` が入力の履歴を持たないと
    /// 決まらない量なので、ここでしか更新できない)。1フレームぶんの `seconds` を
    /// 渡す想定だが、大きくても破綻しない(位相は 0..2π に畳む)。
    pub fn tick(
        &mut self,
        seconds: f32,
        energy: f32,
        cohesion: f32,
        tempo: f32,
    ) -> ResolvedPose {
        self.unattended_seconds += seconds.max(0.0);
        if self.pinned.is_none() {
            let inputs = ExpressionInputs {
                energy,
                cohesion,
                tempo,
                unattended_seconds: self.unattended_seconds,
            };
            self.weights = weights(&inputs);
            self.pose = Pose::mix(&GESTURES, &self.weights);
            self.conflict = conflict_of(&self.pose);
        }
        self.phase = rem_euclidf(self.phase + 2.0 * PI * self.pose.freq * seconds.max(0.0), 2.0 * PI);
        resolve(&self.pose, self.conflict, self.phase)
    }

    /// 位相を固定する(検証のため。`docs/experiments/css-gesture.md` の
    /// 「アニメを 0% に凍結して測る」に相当する)。
    pub fn set_phase(&mut self, phase: f32) {
        self.phase = rem_euclidf(phase, 2.0 * PI);
    }

    pub fn phase(&self) -> f32 {
        self.phase
    }

    pub fn weights(&self) -> [f32; GESTURE_COUNT] {
        self.weights
    }

    pub fn pose(&self) -> Pose {
        self.pose
    }

    /// 混合で捨てられる量 `2*min(lift, sink)`。roll に回る。
    pub fn conflict(&self) -> f32 {
        self.conflict
    }

    pub fn unattended_seconds(&self) -> f32 {
        self.unattended_seconds
    }

    /// いまの重みのまま位相だけを変えた姿勢。
    pub fn resolved_at(&self, phase: f32) -> ResolvedPose {
        resolve(&self.pose, self.conflict, phase)
    }

    /// 重みを1つのジェスチャーに固定する(`--pose <名前>` と、しきい値の実測に使う)。
    /// `tick` を呼んでも固定は解けない(`unpin` するまで)。
    pub fn pin_gesture(&mut self, index: usize) {
        let index = index.min(GESTURE_COUNT - 1);
        let mut weights = [0.0; GESTURE_COUNT];
        weights[index] = 1.0;
        self.weights = weights;
        self.pose = GESTURES[index].pose;
        self.conflict = conflict_of(&self.pose);
        self.pinned = Some(index);
    }

    /// 固定を解いて、状態からの写像に戻す。
    pub fn unpin(&mut self) {
        self.pinned = None;
    }

    /// 固定されているジェスチャーの添字。
    pub fn pinned(&self) -> Option<usize> {
        self.pinned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lively() -> ExpressionInputs {
        ExpressionInputs {
            energy: 1.0,
            cohesion: 1.0,
            tempo: 1.0,
            unattended_seconds: 0.0,
        }
    }

    #[test]
    fn every_gesture_has_a_distinct_name_and_they_can_be_looked_up() {
        // Arrange / Act / Assert
        for (index, gesture) in GESTURES.iter().enumerate() {
            assert_eq!(gesture_index(gesture.name), Some(index));
        }
        assert_eq!(gesture_index("sulk"), None);
    }

    #[test]
    fn the_weights_always_sum_to_one() {
        // Arrange: 入力の端どうしを混ぜても正規化が壊れない
        let cases = [
            lively(),
            ExpressionInputs {
                energy: 0.0,
                cohesion: 0.0,
                tempo: SLOWEST_TEMPO,
                unattended_seconds: 1000.0,
            },
            ExpressionInputs {
                energy: 0.5,
                cohesion: 0.5,
                tempo: 0.8,
                unattended_seconds: 45.0,
            },
        ];

        // Act / Assert
        for inputs in cases {
            let total: f32 = weights(&inputs).iter().sum();
            assert!((total - 1.0).abs() < 1e-6, "got {total} for {inputs:?}");
        }
    }

    #[test]
    fn a_vigorous_pet_just_cared_for_leans_forward() {
        // Arrange / Act
        let weights = weights(&lively());

        // Assert: perk_up か nod が主役になる(沈む側は立たない)
        let top = dominant(&weights);
        assert!(
            GESTURES[top].name == "perk_up" || GESTURES[top].name == "nod",
            "a lively pet must not settle, got {}",
            GESTURES[top].name
        );
        assert!(weights[gesture_index("settle").unwrap()] < 0.2);
        assert!(weights[gesture_index("recoil").unwrap()] < 0.05);
    }

    #[test]
    fn a_neglected_pet_settles_and_a_disappointed_one_recoils() {
        // Arrange: 放置でエネルギーが尽きた個体と、さらにがっかりした個体
        let drained = ExpressionInputs {
            energy: 0.0,
            cohesion: 1.0,
            tempo: 1.0,
            unattended_seconds: 600.0,
        };
        let disappointed = ExpressionInputs {
            tempo: SLOWEST_TEMPO,
            ..drained
        };

        // Act
        let drained_weights = weights(&drained);
        let disappointed_weights = weights(&disappointed);

        // Assert: どちらも主役は沈む側。がっかりが乗ると recoil が立てる
        assert_eq!(
            GESTURES[dominant(&drained_weights)].name,
            "settle",
            "drained: {:?}",
            drained_weights
        );
        let settle = gesture_index("settle").unwrap();
        let recoil = gesture_index("recoil").unwrap();
        assert!(disappointed_weights[recoil] > drained_weights[recoil]);
        assert_eq!(
            GESTURES[dominant(&disappointed_weights)].name,
            "recoil",
            "disappointed: {:?}",
            disappointed_weights
        );
        assert!(disappointed_weights[settle] + disappointed_weights[recoil] > 0.6);
    }

    #[test]
    fn a_scattered_body_shakes_instead_of_nodding() {
        // Arrange: 元気は同じで、まとまりだけを崩す
        let tight = lively();
        let scattered = ExpressionInputs {
            cohesion: 0.3,
            ..lively()
        };

        // Act
        let tight_weights = weights(&tight);
        let scattered_weights = weights(&scattered);

        // Assert: まとまりが崩れると shake が上がり、nod が下がる
        let nod = gesture_index("nod").unwrap();
        let shake = gesture_index("shake").unwrap();
        assert!(scattered_weights[shake] > tight_weights[shake]);
        assert!(scattered_weights[nod] < tight_weights[nod]);
        assert!(scattered_weights[shake] > 0.2);
    }

    #[test]
    fn an_undecided_pet_flickers() {
        // Arrange: 元気が中間で、体がまとまらない
        let undecided = ExpressionInputs {
            energy: 0.5,
            cohesion: 0.2,
            tempo: 1.0,
            unattended_seconds: 10.0,
        };

        // Act
        let weights = weights(&undecided);

        // Assert: 迷いの項が最大になる
        assert_eq!(GESTURES[dominant(&weights)].name, "flicker");
    }

    #[test]
    fn a_single_gesture_mixes_back_to_its_own_pose() {
        // Arrange / Act / Assert: 重みが1点なら補間は恒等
        for (index, gesture) in GESTURES.iter().enumerate() {
            let mut weights = [0.0; GESTURE_COUNT];
            weights[index] = 1.0;
            let mixed = Pose::mix(&GESTURES, &weights);
            assert_eq!(mixed, gesture.pose, "{}", gesture.name);
        }
    }

    #[test]
    fn mixing_never_invents_a_signed_rise() {
        // Arrange: 浮く側(recoil)と沈む側(settle)を等しく混ぜる
        let recoil = gesture_index("recoil").unwrap();
        let settle = gesture_index("settle").unwrap();
        let mut weights = [0.0; GESTURE_COUNT];
        weights[recoil] = 0.5;
        weights[settle] = 0.5;

        // Act
        let mixed = Pose::mix(&GESTURES, &weights);
        let conflict = conflict_of(&mixed);

        // Assert: 符号付きの1次元なら相殺して消える量が、葛藤として残る
        // (lift 0.95 / sink 1.05 の平均 = 0.475 / 0.525 → 2*min = 0.95)
        assert!(
            (conflict - 0.95).abs() < 1e-5,
            "the conflict must survive the mix, got {conflict}"
        );
        // 静的な上下は差だけが残る(0.525 - 0.475 = 0.05 unit 沈む)
        let pose = resolve(&mixed, conflict, 0.0);
        assert!(
            (pose.offset_cells.1 - 0.05 * CELLS_PER_POSE_UNIT).abs() < 1e-5,
            "got {:?}",
            pose.offset_cells
        );
    }

    #[test]
    fn a_single_gesture_has_no_conflict() {
        // Arrange / Act / Assert: 片側しか立てていないので必ず 0
        for (index, gesture) in GESTURES.iter().enumerate() {
            let mut weights = [0.0; GESTURE_COUNT];
            weights[index] = 1.0;
            let conflict = conflict_of(&Pose::mix(&GESTURES, &weights));
            assert!(conflict < 1e-6, "{} has conflict {conflict}", gesture.name);
        }
    }

    #[test]
    fn the_conflict_shows_up_as_roll_not_as_vertical_amplitude() {
        // Arrange: 葛藤を持つ混合と持たない混合
        let recoil = gesture_index("recoil").unwrap();
        let settle = gesture_index("settle").unwrap();
        let mut mixed = [0.0; GESTURE_COUNT];
        mixed[recoil] = 0.5;
        mixed[settle] = 0.5;
        let pose = Pose::mix(&GESTURES, &mixed);
        let conflict = conflict_of(&pose);

        // Act: 位相 π/2 では sin = 1 なので、振れ幅がそのまま出る
        let with_conflict = resolve(&pose, conflict, PI / 2.0);
        let without_conflict = resolve(&pose, 0.0, PI / 2.0);

        // Assert: roll だけが増え、上下の振幅は増えない
        assert!(
            (with_conflict.roll_deg - without_conflict.roll_deg
                - DEG_PER_CONFLICT * conflict)
                .abs()
                < 1e-4
        );
        assert_eq!(
            with_conflict.offset_cells.1,
            without_conflict.offset_cells.1
        );
    }

    #[test]
    fn phase_zero_leaves_only_the_static_pose() {
        // Arrange: 振れを持つジェスチャー(nod)を単独で
        let mut expression = Expression::new();
        expression.pin_gesture(gesture_index("nod").unwrap());

        // Act
        let at_zero = expression.resolved_at(0.0);
        let at_quarter = expression.resolved_at(PI / 2.0);

        // Assert: 位相 0 では振れが消え、tilt_x だけが残る
        assert_eq!(at_zero.pitch_deg, GESTURES[gesture_index("nod").unwrap()].pose.tilt_x);
        assert_eq!(at_zero.roll_deg, 0.0);
        assert_eq!(at_zero.yaw_deg, 0.0);
        assert!((at_quarter.pitch_deg - 18.0).abs() < 1e-4);
    }

    #[test]
    fn ticking_advances_the_phase_by_the_mixed_frequency() {
        // Arrange: idle は freq 1.0
        let mut expression = Expression::new();
        expression.pin_gesture(gesture_index("idle").unwrap());

        // Act
        expression.tick(0.25, 1.0, 1.0, 1.0);

        // Assert: 1周期 1.0 なので、0.25 秒で 1/4 周
        assert!(
            (expression.phase() - PI / 2.0).abs() < 1e-4,
            "got {}",
            expression.phase()
        );
    }

    #[test]
    fn the_phase_wraps_instead_of_growing_without_bound() {
        // Arrange
        let mut expression = Expression::new();
        expression.pin_gesture(gesture_index("recoil").unwrap()); // freq 2.2

        // Act: 十分に長く回す
        for _ in 0..1000 {
            expression.tick(1.0, 0.0, 1.0, 1.0);
        }

        // Assert
        assert!(expression.phase() >= 0.0 && expression.phase() < 2.0 * PI);
    }

    #[test]
    fn a_gesture_can_be_pinned_for_inspection() {
        // Arrange
        let mut expression = Expression::new();

        // Act
        expression.pin_gesture(gesture_index("shake").unwrap());

        // Assert
        assert_eq!(dominant(&expression.weights()), gesture_index("shake").unwrap());
        assert_eq!(expression.pose(), GESTURES[gesture_index("shake").unwrap()].pose);
    }

    #[test]
    fn care_resets_the_unattended_time_that_feeds_perk_up() {
        // Arrange: 放置して perk_up の項を消す
        let mut expression = Expression::new();
        for _ in 0..200 {
            expression.tick(1.0, 1.0, 1.0, 1.0);
        }
        let neglected = expression.weights()[gesture_index("perk_up").unwrap()];
        assert_eq!(neglected, 0.0, "90秒を過ぎたら fresh は 0");

        // Act: かまってもらう
        expression.record_care();
        let resolved = expression.tick(0.0, 1.0, 1.0, 1.0);

        // Assert: perk_up が戻る
        assert!(expression.weights()[gesture_index("perk_up").unwrap()] > 0.3);
        assert_eq!(expression.unattended_seconds(), 0.0);
        assert!(resolved.scale > 1.0, "perk_up は体を大きくする");
    }

    #[test]
    fn the_pose_stays_inside_the_canvas_at_the_worst_case() {
        // Arrange: 上下の移動が最大になる組み合わせ(全部の重みを settle と recoil に置く)
        let mut weights = [0.0; GESTURE_COUNT];
        weights[gesture_index("recoil").unwrap()] = 0.5;
        weights[gesture_index("settle").unwrap()] = 0.5;

        // Act
        let pose = Pose::mix(&GESTURES, &weights);
        let worst = resolve(&pose, conflict_of(&pose), PI / 2.0);

        // Assert: 体の高さ(20セル)に比べて十分小さい。輪郭が画面外へはみ出さない
        assert!(
            worst.offset_cells.1.abs() < 1.5,
            "got {:?}",
            worst.offset_cells
        );
        assert!(worst.scale <= MAX_POSE_SCALE);
    }

    #[test]
    fn resolve_turns_flick_into_a_visibility_that_thins_but_never_vanishes() {
        // Arrange: flicker を単独で置く
        let mut weights = [0.0; GESTURE_COUNT];
        weights[gesture_index("flicker").unwrap()] = 1.0;
        let pose = Pose::mix(&GESTURES, &weights);

        // Act: 明滅の1周期を細かく見る
        let mut lowest = 1.0f32;
        let mut highest = 0.0f32;
        for step in 0..200 {
            let phase = step as f32 / 200.0 * 2.0 * PI;
            let visibility = resolve(&pose, 0.0, phase).visibility;
            lowest = lowest.min(visibility);
            highest = highest.max(visibility);
        }

        // Assert: 薄くはなるが消えはしない(消えると「迷い」ではなく「不在」に見える)
        assert!(
            (lowest - (1.0 - FLICK_DEPTH)).abs() < 1e-4,
            "got {lowest}"
        );
        assert!(highest > 0.99, "got {highest}");
    }
}
