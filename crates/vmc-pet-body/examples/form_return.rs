//! 【実験】生き残る関数で別の形へ移った体に、元の姿へ戻る帰り道はあるか。
//!
//! 「生き残る関数で表情を足せるか」(docs/experiments/expression-axes.md)で、S1s と 2S1v は
//! 生き残る関数 P の中心を −0.25σ へ動かすだけで止まった形(ドーナツ形・歯車形)に移り、P を
//! 戻しても戻らなかった。試したのは「戻す」だけだったので、逆向きのきっかけを与えて元の姿に
//! 戻せるかを確かめる(docs/experiments/expression-axes.md「別の形から戻る帰り道はあるか」)。
//! 戻せるなら、行きと帰りに別のきっかけを使う2状態の表現になる。
//!
//! 1周の流れ(元気な体、成長の強さ 1.0):
//!
//! 1. 行き: P の中心を 750 ステップかけて −0.25σ へ動かし、3000 ステップ保ち、750 ステップで戻す
//! 2. 3000 ステップ保ち、最後の 900 ステップで「移った姿」を測る
//! 3. きっかけ(下記)を 1000 ステップの枠で与える
//! 4. 3000 ステップ保ち、最後の 900 ステップで測り、元の姿・移った姿のどちらに近いかで分ける
//!
//! 1周目で戻ったら、同じことをもう1周繰り返す(1回きりでは表情に使えないため)。行きで形が
//! 変わらなかった試行は数えない。
//!
//! きっかけは、P の一時的な振り、クリック相当の注入に加え、「直接帰らず、ほかの状態を経由する」
//! 経路も試す: エネルギー(成長の強さ)を一時的に下げて戻す、生まれる関数と生き残る関数の中心と
//! 幅をほかの生物(O2u・OG2g)の値へ動かして保ってから戻す。O2u・OG2g・S1s はカーネルが同じ
//! (R=13、リング1本)で中心と幅だけが違うので、S1s ではほかの生物を経由する道になる。2S1v は
//! カーネルが違うので、2S1v のカーネルのまま中心と幅だけを動かす経路になる。
//!
//! 使い方: `cargo run --release -p vmc-pet-body --example form_return -- <出力先>`。
//! 出力先に試行0の姿を PGM で書き出す。ペット本体の振る舞いには触れない。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread;
use std::time::Instant;

use vmc_pet_body::{load_animal, Animal, CellPos, Field, GrowthFunction, Lenia, Perturbation};

const FIELD_SIZE: usize = 32;
const CELLS: usize = FIELD_SIZE * FIELD_SIZE;
const COLLAPSE_MASS: f32 = 5.0;
/// はっきり見える点とみなす値(`persistence_expression.rs` と同じ)。
const CLEARLY_VISIBLE: f32 = 0.2;
const EXPLODED_AREA: usize = CELLS / 2;

const MAX_WARMUP_JITTER: u32 = 300;
const NOISE: f32 = 1e-3;
/// 落ち着かせる長さ(`persistence_expression.rs` のエネルギーを尽きさせる区間と同じ長さ)。
const SETTLE_STEPS: u32 = 2_250;
const RAMP_STEPS: u32 = 750;
const HOLD_STEPS: u32 = 3_000;
const MEASURE_STEPS: u32 = 900;
/// 行きで P の中心をずらす量(σ 単位)。前回、S1s と 2S1v が止まった形に移った値。
const OUTBOUND_SHIFT: f32 = -0.25;
/// きっかけを与える枠の長さ。どのきっかけもこの長さにそろえ、保つ区間の始まりをそろえる。
const TRIGGER_STEPS: u32 = 1_000;
/// P を一時的に振るときの、振り始めてから目標に着くまで・保つ・戻すの長さ。
const PULSE_RAMP_STEPS: u32 = 250;
const PULSE_HOLD_STEPS: u32 = 500;
/// 注入を繰り返す間隔(1秒)。
const CLICK_INTERVAL_STEPS: u32 = 15;
/// 実際のクリックと同じ半径と量(`touch.rs` の `CLICK_BODY_RADIUS_CELLS`・`CLICK_BODY_AMOUNT`)。
const CLICK_RADIUS: f32 = 4.5;
const CLICK_AMOUNT: f32 = 0.20;
/// 元の姿・移った姿のどちらからもこれ以上離れていたら「別の形」とする。前回のゆらぎ
/// (動き差 0.07 以下・形差 0.01 以下)と、行きで生じた差(動き差 1 前後 + 形差 0.4 前後)の間。
const SAME_FORM_DISTANCE: f32 = 0.3;
const TRIALS: u64 = 8;
const CYCLES: usize = 2;
const CODES: [&str; 2] = ["S1s", "2S1v"];

/// 元の姿へ戻すためのきっかけ。
#[derive(Clone, Copy)]
enum Trigger {
    /// 何もしない(移った姿のままでいるかの対照)。
    Wait,
    /// P の中心を `shift` σ・幅を `ratio` 倍へ一時的に振る。
    PersistencePulse { shift: f32, ratio: f32 },
    /// 体の重心から x 方向に `offset` セルずらした位置へ、`amount` を `count` 回注入する。
    Click {
        amount: f32,
        count: u32,
        offset: i32,
    },
    /// 成長の強さ(エネルギー)を `floor` まで一時的に下げてから戻す。放置されて弱り、また
    /// 世話される経路。
    EnergyDip { floor: f32 },
    /// 生まれる関数と生き残る関数の両方を、中心 `center`・幅 `width`(ほかの生物の値)へ、
    /// `fraction` の割合だけ一時的に動かしてから戻す。ほかの生物を経由する経路。カーネルは
    /// 動かさないので、カーネルの違う生物(2S1v)では既存の生物を経由することにはならない。
    SpeciesDetour {
        center: f32,
        width: f32,
        fraction: f32,
    },
}

/// 経由するきっかけで、経由先へ移る・保つ・戻す長さ。経由先で姿が落ち着くまで待つため、ほかの
/// きっかけより長い。保つ区間の最後の `MEASURE_STEPS` で経由中の姿を測る。
const DETOUR_RAMP_STEPS: u32 = 750;
const DETOUR_HOLD_STEPS: u32 = 1_500;

const TRIGGERS: [(&str, Trigger); 18] = [
    ("何もしない", Trigger::Wait),
    (
        "P 中心 +0.25σ に振る",
        Trigger::PersistencePulse {
            shift: 0.25,
            ratio: 1.0,
        },
    ),
    (
        "P 中心 +0.5σ に振る",
        Trigger::PersistencePulse {
            shift: 0.5,
            ratio: 1.0,
        },
    ),
    (
        "P 幅 ×1.1 に振る",
        Trigger::PersistencePulse {
            shift: 0.0,
            ratio: 1.1,
        },
    ),
    (
        "P 幅 ×1.2 に振る",
        Trigger::PersistencePulse {
            shift: 0.0,
            ratio: 1.2,
        },
    ),
    (
        "重心に足す ×1",
        Trigger::Click {
            amount: CLICK_AMOUNT,
            count: 1,
            offset: 0,
        },
    ),
    (
        "重心に足す ×5",
        Trigger::Click {
            amount: CLICK_AMOUNT,
            count: 5,
            offset: 0,
        },
    ),
    (
        "重心に3倍足す ×1",
        Trigger::Click {
            amount: 3.0 * CLICK_AMOUNT,
            count: 1,
            offset: 0,
        },
    ),
    (
        "重心から削る ×5",
        Trigger::Click {
            amount: -CLICK_AMOUNT,
            count: 5,
            offset: 0,
        },
    ),
    (
        "4セル横に足す ×5",
        Trigger::Click {
            amount: CLICK_AMOUNT,
            count: 5,
            offset: 4,
        },
    ),
    (
        "4セル横から削る ×5",
        Trigger::Click {
            amount: -CLICK_AMOUNT,
            count: 5,
            offset: 4,
        },
    ),
    (
        "4セル横に3倍足す ×5",
        Trigger::Click {
            amount: 3.0 * CLICK_AMOUNT,
            count: 5,
            offset: 4,
        },
    ),
    ("エネルギー 0.85 経由", Trigger::EnergyDip { floor: 0.85 }),
    ("エネルギー 0.7 経由", Trigger::EnergyDip { floor: 0.7 }),
    (
        "O2u の μσ 経由",
        Trigger::SpeciesDetour {
            center: 0.15,
            width: 0.015,
            fraction: 1.0,
        },
    ),
    (
        "O2u の μσ へ半分",
        Trigger::SpeciesDetour {
            center: 0.15,
            width: 0.015,
            fraction: 0.5,
        },
    ),
    (
        "OG2g の μσ 経由",
        Trigger::SpeciesDetour {
            center: 0.156,
            width: 0.0224,
            fraction: 1.0,
        },
    ),
    (
        "OG2g の μσ へ半分",
        Trigger::SpeciesDetour {
            center: 0.156,
            width: 0.0224,
            fraction: 0.5,
        },
    ),
];

/// 外部クレートに頼らない決定的な疑似乱数(xorshift64)。
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn unit(&mut self) -> f32 {
        (self.next_u64() >> 11) as f32 / (1u64 << 53) as f32
    }

    fn range_f32(&mut self, low: f32, high: f32) -> f32 {
        low + self.unit() * (high - low)
    }

    fn range_u32(&mut self, low: u32, high: u32) -> u32 {
        low + (self.next_u64() % (high - low + 1) as u64) as u32
    }
}

/// 生物と試行番号だけから種を作る。きっかけによらず、行きまでの軌跡をそろえる。
fn seed_for(code: &str, trial: u64) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in code.bytes().chain(trial.to_le_bytes()) {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash | 1
}

/// `base` の中心を `shift` σ・幅を `ratio` 倍へ、`progress`(0..=1)の割合だけ動かした関数。
fn shifted(base: GrowthFunction, shift: f32, ratio: f32, progress: f32) -> GrowthFunction {
    GrowthFunction {
        mapping: base.mapping,
        center: base.center + shift * base.width * progress,
        width: base.width * (1.0 + (ratio - 1.0) * progress),
    }
}

/// 60秒ぶんの見た目の特徴(`persistence_expression.rs` と同じ定義)。
#[derive(Clone, Copy, Default)]
struct Features {
    mean_speed: f32,
    mass_deviation: f32,
    visible_area: f32,
}

fn toroidal_offset(offset: f32) -> f32 {
    let size = FIELD_SIZE as f32;
    let offset = offset.rem_euclid(size);
    if offset > size / 2.0 {
        offset - size
    } else {
        offset
    }
}

fn visible_area(field: &Field) -> usize {
    let view = field.view();
    (0..CELLS)
        .filter(|index| view.get(index % FIELD_SIZE, index / FIELD_SIZE) > CLEARLY_VISIBLE)
        .count()
}

#[derive(Default)]
struct FeatureRecorder {
    previous_centroid: Option<(f32, f32)>,
    path: f32,
    moves: u32,
    masses: Vec<f32>,
    area_total: f32,
}

impl FeatureRecorder {
    fn record(&mut self, field: &Field) {
        let centroid = field.view().toroidal_centroid();
        if let (Some(a), Some(b)) = (self.previous_centroid, centroid) {
            let (dx, dy) = (toroidal_offset(b.0 - a.0), toroidal_offset(b.1 - a.1));
            self.path += (dx * dx + dy * dy).sqrt();
            self.moves += 1;
        }
        self.previous_centroid = centroid;
        self.masses.push(field.mass());
        self.area_total += visible_area(field) as f32;
    }

    fn finish(&self) -> Features {
        let count = self.masses.len().max(1) as f32;
        let mean_mass = self.masses.iter().sum::<f32>() / count;
        let variance = self
            .masses
            .iter()
            .map(|mass| (mass - mean_mass) * (mass - mean_mass))
            .sum::<f32>()
            / count;
        Features {
            mean_speed: self.path / self.moves.max(1) as f32,
            mass_deviation: variance.sqrt(),
            visible_area: self.area_total / count,
        }
    }
}

fn relative_difference(a: f32, b: f32) -> f32 {
    let scale = a.abs().max(b.abs());
    if scale <= 1e-6 {
        0.0
    } else {
        (a - b).abs() / scale
    }
}

/// 動き差(いまの `legibility` と同じ式)と形差(はっきり見える点の数の相対差)の和。
fn distance(a: &Features, b: &Features) -> f32 {
    let motion = (relative_difference(a.mean_speed, b.mean_speed)
        + relative_difference(a.mass_deviation, b.mass_deviation))
        / 2.0;
    motion + relative_difference(a.visible_area, b.visible_area)
}

fn write_pgm(path: &Path, field: &Field) {
    let view = field.view();
    let scale = 4;
    let side = FIELD_SIZE * scale;
    let mut bytes = format!("P5\n{side} {side}\n255\n").into_bytes();
    for y in 0..side {
        for x in 0..side {
            let value = view.get(x / scale, y / scale).clamp(0.0, 1.0);
            bytes.push((value * 255.0).round() as u8);
        }
    }
    fs::write(path, bytes).expect("PGM を書き出せない");
}

/// 1周の結果。
#[derive(Clone, Copy, PartialEq)]
enum Verdict {
    /// 行きで形が変わらなかった(数えない)。
    NotTransformed,
    Returned,
    Stayed,
    OtherForm,
    Collapsed,
    Exploded,
}

/// 体が壊れたら、その区間の名前と一緒に知らせる。
enum Broken {
    Collapsed,
    Exploded,
}

struct Body {
    lenia: Lenia,
    field: Field,
    base: GrowthFunction,
}

impl Body {
    /// 生き残る関数を `persistence` にして1ステップ進める。壊れたら `Err`。
    fn step(&mut self, persistence: GrowthFunction) -> Result<(), Broken> {
        self.step_rule(self.base, persistence, 1.0)
    }

    /// 生まれる関数・生き残る関数・成長の強さを指定して1ステップ進める。壊れたら `Err`。
    fn step_rule(
        &mut self,
        genesis: GrowthFunction,
        persistence: GrowthFunction,
        growth_scale: f32,
    ) -> Result<(), Broken> {
        self.lenia
            .step_glaberish(&mut self.field, genesis, persistence, growth_scale, 1.0);
        if self.field.mass() < COLLAPSE_MASS {
            return Err(Broken::Collapsed);
        }
        if visible_area(&self.field) > EXPLODED_AREA {
            return Err(Broken::Exploded);
        }
        Ok(())
    }

    /// `steps` ステップ、P を元のまま保ち、最後の `MEASURE_STEPS` で特徴を測る。
    fn hold_and_measure(&mut self, steps: u32) -> Result<Features, Broken> {
        let mut recorder = FeatureRecorder::default();
        for step in 0..steps {
            self.step(self.base)?;
            if step >= steps - MEASURE_STEPS {
                recorder.record(&self.field);
            }
        }
        Ok(recorder.finish())
    }

    /// 行き: P の中心を `OUTBOUND_SHIFT` へ動かして保ち、元に戻す。
    fn outbound(&mut self) -> Result<(), Broken> {
        for step in 0..RAMP_STEPS {
            let progress = step as f32 / RAMP_STEPS as f32;
            self.step(shifted(self.base, OUTBOUND_SHIFT, 1.0, progress))?;
        }
        for _ in 0..HOLD_STEPS {
            self.step(shifted(self.base, OUTBOUND_SHIFT, 1.0, 1.0))?;
        }
        for step in 0..RAMP_STEPS {
            let progress = 1.0 - step as f32 / RAMP_STEPS as f32;
            self.step(shifted(self.base, OUTBOUND_SHIFT, 1.0, progress))?;
        }
        Ok(())
    }

    /// 経由するきっかけを与える。経由先へ移り、保ち、戻す。保つ区間の最後の `MEASURE_STEPS` で
    /// 経由中の姿を測って返し、そのときの場を `at_midpoint` に渡す。
    fn detour(
        &mut self,
        trigger: Trigger,
        at_midpoint: &dyn Fn(&Field),
    ) -> Result<Features, Broken> {
        let mut recorder = FeatureRecorder::default();
        let total = 2 * DETOUR_RAMP_STEPS + DETOUR_HOLD_STEPS;
        for step in 0..total {
            let progress = if step < DETOUR_RAMP_STEPS {
                step as f32 / DETOUR_RAMP_STEPS as f32
            } else if step < DETOUR_RAMP_STEPS + DETOUR_HOLD_STEPS {
                1.0
            } else {
                let back = step - DETOUR_RAMP_STEPS - DETOUR_HOLD_STEPS;
                1.0 - back as f32 / DETOUR_RAMP_STEPS as f32
            };
            let (rule, growth_scale) = match trigger {
                Trigger::EnergyDip { floor } => (self.base, 1.0 - (1.0 - floor) * progress),
                Trigger::SpeciesDetour {
                    center,
                    width,
                    fraction,
                } => {
                    let moved = progress * fraction;
                    let rule = GrowthFunction {
                        mapping: self.base.mapping,
                        center: self.base.center + (center - self.base.center) * moved,
                        width: self.base.width + (width - self.base.width) * moved,
                    };
                    (rule, 1.0)
                }
                _ => unreachable!("経由するきっかけだけを渡す"),
            };
            self.step_rule(rule, rule, growth_scale)?;
            let hold_end = DETOUR_RAMP_STEPS + DETOUR_HOLD_STEPS;
            if (hold_end - MEASURE_STEPS..hold_end).contains(&step) {
                recorder.record(&self.field);
            }
            if step + 1 == hold_end {
                at_midpoint(&self.field);
            }
        }
        Ok(recorder.finish())
    }

    /// きっかけを与える。経由するきっかけなら経由中の姿を返す。
    fn trigger(
        &mut self,
        trigger: Trigger,
        at_midpoint: &dyn Fn(&Field),
    ) -> Result<Option<Features>, Broken> {
        if matches!(
            trigger,
            Trigger::EnergyDip { .. } | Trigger::SpeciesDetour { .. }
        ) {
            return self.detour(trigger, at_midpoint).map(Some);
        }
        for step in 0..TRIGGER_STEPS {
            let persistence = match trigger {
                Trigger::PersistencePulse { shift, ratio } => {
                    let progress = if step < PULSE_RAMP_STEPS {
                        step as f32 / PULSE_RAMP_STEPS as f32
                    } else if step < PULSE_RAMP_STEPS + PULSE_HOLD_STEPS {
                        1.0
                    } else {
                        let back = step - PULSE_RAMP_STEPS - PULSE_HOLD_STEPS;
                        (1.0 - back as f32 / PULSE_RAMP_STEPS as f32).max(0.0)
                    };
                    shifted(self.base, shift, ratio, progress)
                }
                Trigger::Wait
                | Trigger::Click { .. }
                | Trigger::EnergyDip { .. }
                | Trigger::SpeciesDetour { .. } => self.base,
            };
            if let Trigger::Click {
                amount,
                count,
                offset,
            } = trigger
            {
                let clicking = step.is_multiple_of(CLICK_INTERVAL_STEPS)
                    && step / CLICK_INTERVAL_STEPS < count;
                if clicking {
                    self.click(amount, offset);
                }
            }
            self.step(persistence)?;
        }
        Ok(None)
    }

    fn click(&mut self, amount: f32, offset: i32) {
        let Some((x, y)) = self.field.view().toroidal_centroid() else {
            return;
        };
        let size = FIELD_SIZE as i32;
        let at = CellPos {
            x: ((x.round() as i32 + offset).rem_euclid(size)) as usize,
            y: (y.round() as i32).rem_euclid(size) as usize,
        };
        self.field.inject(&Perturbation {
            at,
            radius: CLICK_RADIUS,
            amount,
        });
    }
}

fn verdict_of(broken: Broken) -> Verdict {
    match broken {
        Broken::Collapsed => Verdict::Collapsed,
        Broken::Exploded => Verdict::Exploded,
    }
}

struct Outcome {
    verdicts: Vec<Verdict>,
    /// 各周の (元の姿との距離, 移った姿との距離)。
    distances: Vec<(f32, f32)>,
    /// 各周の (元の姿の速さ, きっかけの後の速さ)。「別の形」が動き出したのか、止まったまま
    /// 少し変わっただけなのかを見分けるため。
    speeds: Vec<(f32, f32)>,
    /// 各周の、経由中の速さ(経由するきっかけのときだけ)。経由先で動き出していたかを見る。
    detour_speeds: Vec<f32>,
}

fn run(
    animal: &Animal,
    trigger: Trigger,
    trigger_label: &str,
    trial: u64,
    output: Option<&Path>,
) -> Outcome {
    let base = animal.params.growth_function();
    let mut rng = Rng(seed_for(&animal.code, trial));
    let mut body = Body {
        lenia: Lenia::new(animal.params.clone()),
        field: Field::new(FIELD_SIZE, FIELD_SIZE),
        base,
    };
    body.field.place_centered(&animal.pattern);
    let snapshot = |field: &Field, moment: &str| {
        if let Some(output) = output {
            let name = format!("{}_{trigger_label}_{moment}.pgm", animal.code);
            write_pgm(&output.join(name.replace(' ', "_")), field);
        }
    };
    let mut outcome = Outcome {
        verdicts: Vec::new(),
        distances: Vec::new(),
        speeds: Vec::new(),
        detour_speeds: Vec::new(),
    };
    for _ in 0..rng.range_u32(60, 60 + MAX_WARMUP_JITTER) {
        let _ = body.step(base);
    }
    let noise: Vec<f32> = (0..CELLS)
        .map(|_| 1.0 + rng.range_f32(-NOISE, NOISE))
        .collect();
    body.field
        .map(|x, y, value| (value * noise[y * FIELD_SIZE + x]).min(1.0));

    let original = match body.hold_and_measure(SETTLE_STEPS) {
        Ok(features) => features,
        Err(broken) => {
            outcome.verdicts.push(verdict_of(broken));
            return outcome;
        }
    };
    snapshot(&body.field, "0original");

    for cycle in 0..CYCLES {
        let transformed = match body
            .outbound()
            .and_then(|_| body.hold_and_measure(HOLD_STEPS))
        {
            Ok(features) => features,
            Err(broken) => {
                outcome.verdicts.push(verdict_of(broken));
                return outcome;
            }
        };
        snapshot(&body.field, &format!("{cycle}_1transformed"));
        if distance(&transformed, &original) < SAME_FORM_DISTANCE {
            outcome.verdicts.push(Verdict::NotTransformed);
            return outcome;
        }
        let midpoint_moment = format!("{cycle}_2detour");
        let at_midpoint = |field: &Field| snapshot(field, &midpoint_moment);
        let triggered = body.trigger(trigger, &at_midpoint);
        let after = match triggered.and_then(|detour| {
            if let Some(features) = detour {
                outcome.detour_speeds.push(features.mean_speed);
            }
            body.hold_and_measure(HOLD_STEPS)
        }) {
            Ok(features) => features,
            Err(broken) => {
                outcome.verdicts.push(verdict_of(broken));
                return outcome;
            }
        };
        snapshot(&body.field, &format!("{cycle}_2after"));
        let to_original = distance(&after, &original);
        let to_transformed = distance(&after, &transformed);
        outcome.distances.push((to_original, to_transformed));
        outcome.speeds.push((original.mean_speed, after.mean_speed));
        let verdict = if to_original < to_transformed && to_original < SAME_FORM_DISTANCE {
            Verdict::Returned
        } else if to_transformed <= to_original && to_transformed < SAME_FORM_DISTANCE {
            Verdict::Stayed
        } else {
            Verdict::OtherForm
        };
        outcome.verdicts.push(verdict);
        if verdict != Verdict::Returned {
            return outcome;
        }
    }
    outcome
}

fn count(outcomes: &[Outcome], cycle: usize, verdict: Verdict) -> usize {
    outcomes
        .iter()
        .filter(|o| o.verdicts.get(cycle) == Some(&verdict))
        .count()
}

fn main() {
    let output = PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("出力先のディレクトリを引数で渡す"),
    );
    fs::create_dir_all(&output).expect("出力先を作れない");
    let started = Instant::now();
    let animals: Vec<Animal> = CODES
        .iter()
        .map(|code| load_animal(code).unwrap())
        .collect();

    let jobs: Vec<(usize, usize, u64)> = (0..animals.len())
        .flat_map(|a| (0..TRIGGERS.len()).flat_map(move |t| (0..TRIALS).map(move |r| (a, t, r))))
        .collect();
    let job_count = jobs.len();
    let queue = Mutex::new(jobs.into_iter().enumerate());
    let results: Mutex<Vec<Option<Outcome>>> = Mutex::new((0..job_count).map(|_| None).collect());
    let workers = thread::available_parallelism().map_or(4, |n| n.get());
    thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let Some((index, (animal_index, trigger_index, trial))) =
                    queue.lock().unwrap().next()
                else {
                    break;
                };
                let (label, trigger) = TRIGGERS[trigger_index];
                let outcome = run(
                    &animals[animal_index],
                    trigger,
                    label,
                    trial,
                    (trial == 0).then_some(output.as_path()),
                );
                results.lock().unwrap()[index] = Some(outcome);
            });
        }
    });
    let results: Vec<Outcome> = results
        .into_inner()
        .unwrap()
        .into_iter()
        .map(Option::unwrap)
        .collect();

    println!(
        "場 {FIELD_SIZE}×{FIELD_SIZE}、元気な体、各きっかけ {TRIALS} 回。行き = P 中心 {OUTBOUND_SHIFT}σ。"
    );
    println!("距離 = 動き差 + 形差。どちらの姿からも {SAME_FORM_DISTANCE} 以上離れたら「別の形」");
    for (animal_index, animal) in animals.iter().enumerate() {
        println!();
        println!("==== {} ====", animal.code);
        println!(
            "  {:22} | 変わらず | 1周目: 戻った 移ったまま 別の形 崩壊 膨張 | 2周目: 戻った 移ったまま 別の形 崩壊 膨張 | 1周目の距離(元/移った) の平均 | 速さ(元の平均 / 後の最大) | 経由中の速さ(最大)",
            "きっかけ"
        );
        for (trigger_index, (label, _)) in TRIGGERS.iter().enumerate() {
            let start = (animal_index * TRIGGERS.len() + trigger_index) * TRIALS as usize;
            let outcomes = &results[start..start + TRIALS as usize];
            let first_distances: Vec<(f32, f32)> = outcomes
                .iter()
                .filter_map(|o| o.distances.first().copied())
                .collect();
            let mean_distance = if first_distances.is_empty() {
                "-".to_string()
            } else {
                let n = first_distances.len() as f32;
                let (to_original, to_transformed) = first_distances
                    .iter()
                    .fold((0.0, 0.0), |(a, b), (x, y)| (a + x, b + y));
                format!("{:.2} / {:.2}", to_original / n, to_transformed / n)
            };
            let cycle_cells = |cycle: usize| {
                format!(
                    "{:>6} {:>10} {:>6} {:>4} {:>4}",
                    count(outcomes, cycle, Verdict::Returned),
                    count(outcomes, cycle, Verdict::Stayed),
                    count(outcomes, cycle, Verdict::OtherForm),
                    count(outcomes, cycle, Verdict::Collapsed),
                    count(outcomes, cycle, Verdict::Exploded)
                )
            };
            let detour = outcomes
                .iter()
                .filter_map(|o| o.detour_speeds.first().copied())
                .fold(None, |max: Option<f32>, speed| {
                    Some(max.map_or(speed, |m| m.max(speed)))
                })
                .map_or("-".to_string(), |speed| format!("{speed:.3}"));
            let first_speeds: Vec<(f32, f32)> = outcomes
                .iter()
                .filter_map(|o| o.speeds.first().copied())
                .collect();
            let speeds = if first_speeds.is_empty() {
                "-".to_string()
            } else {
                let original_mean =
                    first_speeds.iter().map(|(o, _)| o).sum::<f32>() / first_speeds.len() as f32;
                let after_max = first_speeds.iter().map(|(_, a)| *a).fold(0.0, f32::max);
                format!("{original_mean:.3} / {after_max:.3}")
            };
            println!(
                "  {label:22} | {:>8} | {} | {} | {mean_distance} | {speeds} | {detour}",
                outcomes
                    .iter()
                    .filter(|o| o.verdicts.first() == Some(&Verdict::NotTransformed))
                    .count(),
                cycle_cells(0),
                cycle_cells(1)
            );
        }
    }
    println!();
    println!("合計 {:.0} 秒", started.elapsed().as_secs_f32());
}
