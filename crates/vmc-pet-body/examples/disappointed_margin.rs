//! 【実験】放置されてがっかりした体は、崩壊までどれだけ余裕があるか。
//!
//! 「成長の強さの崖は時間の刻みから来ているか」(docs/experiments/rule-candidates.md)で、
//! O2u の崖はテンポを遅くするほど上がり、×0.5 で 0.844 だった。放置されて成長の強さが下限 0.85
//! まで落ち、がっかりしてテンポが ×0.6 になった O2u は、置いた直後から急に弱める条件では崖から
//! 0.01〜0.02 しか離れていない。実際の使われ方に近い条件で余裕を測り直す
//! (docs/experiments/rule-candidates.md「放置されてがっかりした体の余裕」)。
//!
//! - **測定1(いまのペットそのもの)**: `Pet`(自律コントローラを含む)を放置してエネルギーを
//!   自然に尽きさせ、がっかりを徐々に最大まで上げ(テンポ ×0.6)、そのまま20000ステップ。
//!   落ち着かせる長さを乱数でずらして16回試し、崩壊した回数を数える
//! - **測定2(徐々に弱らせたときの崖)**: `Lenia` を直接進め、成長の強さを徐々に目標の下限まで、
//!   テンポを徐々に目標まで下げて保つ。場にごく小さなゆらぎを掛けて何回も試し、崩壊した割合を
//!   下限ごとに数える。いまの下限 0.85 が崖からどれだけ離れているかが分かる
//!
//! ペット本体の振る舞いには触れない。

use std::thread;
use std::time::Instant;

use vmc_pet_body::{load_animal, Animal, Field, Lenia, Pet, PetMemory};

const FIELD_SIZE: usize = 32;
const CELLS: usize = FIELD_SIZE * FIELD_SIZE;
/// `Pet::step` と同じ崩壊の判定。
const COLLAPSE_MASS: f32 = 5.0;
/// 触られないままエネルギーが尽きるまでのステップ数(`LeniaBody` の減り方と同じ約150秒)。
const NEGLECT_STEPS: u32 = 2_250;
/// エネルギーが尽きてから、がっかり(テンポ)を最大まで上げるのにかけるステップ数。
const MOOD_RAMP_STEPS: u32 = 750;
/// 落ち着かせる長さを乱数でずらす幅(試行ごとに違う軌跡にするため)。
const MAX_WARMUP_JITTER: u32 = 300;

const PET_HOLD_STEPS: u32 = 20_000;
const PET_TRIALS: u64 = 16;

const LENIA_WARMUP_STEPS: u32 = 60;
const LENIA_HOLD_STEPS: u32 = 6_000;
/// 各セルに掛けるゆらぎの幅。
const NOISE: f32 = 1e-3;
const O2U_FLOORS: [f32; 6] = [0.86, 0.85, 0.84, 0.83, 0.82, 0.80];
const O2U_TEMPOS: [f32; 3] = [1.0, 0.6, 0.5];
const O2U_TRIALS: u64 = 16;
const ALL_FLOORS: [f32; 4] = [0.85, 0.83, 0.80, 0.77];
/// がっかりしきったときのテンポ(`mood::DISAPPOINTED_TEMPO` と同じ)。
const DISAPPOINTED_TEMPO: f32 = 0.6;
const ALL_TRIALS: u64 = 8;

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

fn seed_for(text: &str, number: u64) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes().chain(number.to_le_bytes()) {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash | 1
}

/// エネルギーが尽きた後に、がっかりを 0 から 1 へ上げていく割合。
fn mood_onset(step: u32) -> f32 {
    (step.saturating_sub(NEGLECT_STEPS) as f32 / MOOD_RAMP_STEPS as f32).min(1.0)
}

struct PetOutcome {
    /// 崩壊したステップ(放置し始めてから)。生き延びたら `None`。
    collapsed_at: Option<u32>,
    /// 放置し始めたときの総量に対する、いちばん低かった総量の割合。
    lowest_mass_ratio: f32,
}

/// 測定1: いまのペットを放置してがっかりさせ続ける。
fn pet_trial(code: &str, trial: u64) -> PetOutcome {
    let mut rng = Rng(seed_for(code, trial));
    let mut pet = Pet::load(code, FIELD_SIZE, FIELD_SIZE).unwrap();
    for _ in 0..rng.range_u32(0, MAX_WARMUP_JITTER) {
        pet.restore(PetMemory::with_energy(1.0), 0.0);
        pet.step();
    }
    let reference = pet.mass();
    let mut lowest_mass_ratio = f32::INFINITY;
    for step in 0..(NEGLECT_STEPS + MOOD_RAMP_STEPS + PET_HOLD_STEPS) {
        pet.pin_mood(0.0, mood_onset(step));
        if pet.step() {
            return PetOutcome {
                collapsed_at: Some(step),
                lowest_mass_ratio,
            };
        }
        lowest_mass_ratio = lowest_mass_ratio.min(pet.mass() / reference);
    }
    PetOutcome {
        collapsed_at: None,
        lowest_mass_ratio,
    }
}

/// 測定2: 成長の強さを `growth_floor` まで、テンポを `tempo_floor` まで徐々に下げて保つ。
/// 崩壊したら `true`。
fn lenia_trial(animal: &Animal, growth_floor: f32, tempo_floor: f32, seed: u64) -> bool {
    let mut rng = Rng(seed);
    let mut lenia = Lenia::new(animal.params.clone());
    let mut field = Field::new(FIELD_SIZE, FIELD_SIZE);
    field.place_centered(&animal.pattern);
    for _ in 0..(LENIA_WARMUP_STEPS + rng.range_u32(0, MAX_WARMUP_JITTER)) {
        lenia.step(&mut field, 1.0);
    }
    let noise: Vec<f32> = (0..CELLS)
        .map(|_| 1.0 + rng.range_f32(-NOISE, NOISE))
        .collect();
    field.map(|x, y, value| (value * noise[y * FIELD_SIZE + x]).min(1.0));
    for step in 0..(NEGLECT_STEPS + MOOD_RAMP_STEPS + LENIA_HOLD_STEPS) {
        let depletion = (step as f32 / NEGLECT_STEPS as f32).min(1.0);
        let growth = 1.0 - (1.0 - growth_floor) * depletion;
        let tempo = 1.0 - (1.0 - tempo_floor) * mood_onset(step);
        lenia.step_at_tempo(&mut field, growth, tempo);
        if field.mass() < COLLAPSE_MASS {
            return true;
        }
    }
    false
}

fn collapses(code: &str, growth_floor: f32, tempo_floor: f32, trials: u64) -> u64 {
    let animal = load_animal(code).unwrap();
    let label = format!("{code}-{growth_floor}-{tempo_floor}");
    (0..trials)
        .filter(|trial| lenia_trial(&animal, growth_floor, tempo_floor, seed_for(&label, *trial)))
        .count() as u64
}

fn main() {
    let started = Instant::now();
    let codes: Vec<String> = vmc_pet_body::list_animals()
        .unwrap()
        .into_iter()
        .map(|(code, _)| code)
        .collect();

    let (pet_results, o2u_results, all_results) = thread::scope(|scope| {
        let pet_handles: Vec<_> = codes
            .iter()
            .map(|code| {
                scope.spawn(move || {
                    let outcomes: Vec<PetOutcome> = (0..PET_TRIALS)
                        .map(|trial| pet_trial(code, trial))
                        .collect();
                    (code.clone(), outcomes)
                })
            })
            .collect();
        let o2u_handles: Vec<_> = O2U_TEMPOS
            .iter()
            .map(|tempo| {
                scope.spawn(move || {
                    let counts: Vec<u64> = O2U_FLOORS
                        .iter()
                        .map(|floor| collapses("O2u", *floor, *tempo, O2U_TRIALS))
                        .collect();
                    (*tempo, counts)
                })
            })
            .collect();
        let all_handles: Vec<_> = codes
            .iter()
            .map(|code| {
                scope.spawn(move || {
                    let counts: Vec<u64> = ALL_FLOORS
                        .iter()
                        .map(|floor| collapses(code, *floor, DISAPPOINTED_TEMPO, ALL_TRIALS))
                        .collect();
                    (code.clone(), counts)
                })
            })
            .collect();
        let join = |handle: thread::ScopedJoinHandle<'_, _>| handle.join().unwrap();
        (
            pet_handles.into_iter().map(join).collect::<Vec<_>>(),
            o2u_handles
                .into_iter()
                .map(|h| h.join().unwrap())
                .collect::<Vec<_>>(),
            all_handles
                .into_iter()
                .map(|h| h.join().unwrap())
                .collect::<Vec<_>>(),
        )
    });

    println!(
        "==== 測定1: いまのペットを放置してがっかりさせ続ける({PET_TRIALS}回、放置 {NEGLECT_STEPS} + がっかりへ {MOOD_RAMP_STEPS} + {PET_HOLD_STEPS} ステップ) ===="
    );
    for (code, outcomes) in &pet_results {
        let collapsed: Vec<u32> = outcomes.iter().filter_map(|o| o.collapsed_at).collect();
        let lowest = outcomes
            .iter()
            .map(|o| o.lowest_mass_ratio)
            .fold(f32::INFINITY, f32::min);
        println!(
            "  {code:6} 崩壊 {}/{PET_TRIALS}{} | 総量の底(放置し始めを1として、全試行で最も低い) {lowest:.2}",
            collapsed.len(),
            if collapsed.is_empty() {
                String::new()
            } else {
                format!("(ステップ {collapsed:?})")
            }
        );
    }
    println!();

    println!(
        "==== 測定2: O2u を徐々に弱らせたときに崩壊した回数({O2U_TRIALS}回中。行=テンポの下限、列=成長の強さの下限) ===="
    );
    let header: Vec<String> = O2U_FLOORS.iter().map(|f| format!("{f:.2}")).collect();
    println!("  テンポ  | {}", header.join(" | "));
    for (tempo, counts) in &o2u_results {
        let cells: Vec<String> = counts.iter().map(|c| format!("{c:4}")).collect();
        println!("  ×{tempo:<5} | {}", cells.join(" | "));
    }
    println!();

    println!(
        "==== 測定2: 全生物、テンポ ×{DISAPPOINTED_TEMPO} まで徐々に弱らせたときに崩壊した回数({ALL_TRIALS}回中) ===="
    );
    let header: Vec<String> = ALL_FLOORS.iter().map(|f| format!("{f:.2}")).collect();
    println!("  生物   | {}", header.join(" | "));
    for (code, counts) in &all_results {
        let cells: Vec<String> = counts.iter().map(|c| format!("{c:4}")).collect();
        println!("  {code:6} | {}", cells.join(" | "));
    }
    println!();
    println!("合計 {:.0} 秒", started.elapsed().as_secs_f32());
}
