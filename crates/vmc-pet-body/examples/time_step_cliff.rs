//! 【実験】成長の強さの崖は、Lenia の時間の刻み(離散時間ステップ)から来ているのか。
//!
//! issue #1「体(Lenia)を別のルールに差し替える候補の検討」は、これまでの実験で出会った
//! 崖(growth_scale 0.78 で生き延び、0.77 で完全崩壊)が離散時間ステップ由来の不安定性かも
//! しれず、時間を連続に近づけた Asymptotic Lenia なら崖がなだらかになるかもしれない、と
//! 提案している。ルールを実装する前に、今の Lenia のまま時間の刻みを小さくして崖を測り直す
//! (docs/DESIGN.md「成長の強さの崖は時間の刻みから来ているか」)。
//!
//! - 時間の刻みはテンポ(`Lenia::step_at_tempo`)で変える。同じ「体の時間」だけ進めて比べる
//!   (テンポ ×0.5 なら2倍のステップ数)
//! - 条件は `lenia::tests::growth_scale_has_a_cliff_rather_than_a_gentle_slope` と同じ
//!   (64×64 の場、置いた直後から成長を弱める)。体の時間は、崖の近くでゆっくり崩れる分を
//!   拾えるよう 1800(テンポ ×1 の 1800 ステップ)にした
//! - 崖の位置: 0.50〜1.00 を 0.05 刻みで生き延びるか調べ、一番高い崩壊と一番低い生存の間を
//!   8回二分して狭める
//! - 崖の鋭さ: 崖のすぐ上での総量を、同じテンポの健康な総量と比べる。すぐ上でも総量が
//!   高ければ崖は急で、ゼロに近づいていれば、なだらかに移り変わっている
//!
//! 刻みを小さくすると崖が動く・なだらかになるなら、時間ステップ由来の仮説が支持される。
//! 変わらないなら、崖は生物がそのパラメータで存在できなくなる境目(分岐)の性質である。

use std::thread;
use std::time::Instant;

use vmc_pet_body::{load_animal, Animal, Field, Lenia};

/// 既存の崖のテストと同じ場の大きさ。
const FIELD_SIZE: usize = 64;
/// `Pet::step` と同じ崩壊の判定。
const COLLAPSE_MASS: f32 = 5.0;
/// 比べる体の時間(テンポ ×1 のステップ数に相当)。
const BODY_TIME: f32 = 1_800.0;
const TEMPOS: [f32; 5] = [2.0, 1.0, 0.5, 0.25, 0.1];
const BISECTIONS: usize = 8;
/// 崖のすぐ上で総量を見る幅。
const ABOVE_CLIFF: [f32; 4] = [0.005, 0.01, 0.02, 0.05];
/// 崩れる速さを見る、崖のすぐ下の幅。
const BELOW_CLIFF: f32 = 0.01;

struct Outcome {
    /// 崩壊した体の時間。生き延びたら `None`。
    collapsed_at: Option<f32>,
    final_mass: f32,
}

fn run(animal: &Animal, growth_scale: f32, tempo: f32, body_time: f32) -> Outcome {
    let mut field = Field::new(FIELD_SIZE, FIELD_SIZE);
    field.place_centered(&animal.pattern);
    let mut lenia = Lenia::new(animal.params.clone());
    let steps = (body_time / tempo).round() as u32;
    for step in 0..steps {
        lenia.step_at_tempo(&mut field, growth_scale, tempo);
        if field.mass() < COLLAPSE_MASS {
            return Outcome {
                collapsed_at: Some((step + 1) as f32 * tempo),
                final_mass: field.mass(),
            };
        }
    }
    Outcome {
        collapsed_at: None,
        final_mass: field.mass(),
    }
}

/// 崖の位置を二分で狭めた結果。
enum Cliff {
    /// `collapses` で崩壊し、`survives` で生き延びる(その間に崖がある)。
    Between { collapses: f32, survives: f32 },
    /// 調べた範囲では全部生き延びた。
    NeverCollapses,
    /// 調べた範囲では全部崩壊した。
    AlwaysCollapses,
}

struct Measurement {
    tempo: f32,
    cliff: Cliff,
    /// 粗く調べたとき、生き延びる強さより上で崩壊した(崖が1つでない)か。
    non_monotone: bool,
    healthy_mass: f32,
    /// 崖のすぐ上での、健康な総量に対する割合。
    relative_mass_above: Vec<f32>,
    /// 崖のすぐ下で崩壊するまでの体の時間。
    collapse_time_below: Option<f32>,
}

fn measure(animal: &Animal, tempo: f32) -> Measurement {
    let survives = |scale: f32| run(animal, scale, tempo, BODY_TIME).collapsed_at.is_none();
    let coarse: Vec<(f32, bool)> = (0..=10)
        .map(|i| {
            let scale = 0.50 + 0.05 * i as f32;
            (scale, survives(scale))
        })
        .collect();
    let lowest_survivor = coarse.iter().find(|(_, alive)| *alive).map(|(s, _)| *s);
    let non_monotone =
        lowest_survivor.is_some_and(|low| coarse.iter().any(|(s, alive)| *s > low && !alive));
    let healthy_mass = run(animal, 1.0, tempo, BODY_TIME).final_mass;

    let Some(lowest_survivor) = lowest_survivor else {
        return Measurement {
            tempo,
            cliff: Cliff::AlwaysCollapses,
            non_monotone,
            healthy_mass,
            relative_mass_above: Vec::new(),
            collapse_time_below: None,
        };
    };
    if lowest_survivor <= 0.50 {
        return Measurement {
            tempo,
            cliff: Cliff::NeverCollapses,
            non_monotone,
            healthy_mass,
            relative_mass_above: Vec::new(),
            collapse_time_below: None,
        };
    }
    let (mut low, mut high) = (lowest_survivor - 0.05, lowest_survivor);
    for _ in 0..BISECTIONS {
        let middle = (low + high) / 2.0;
        if survives(middle) {
            high = middle;
        } else {
            low = middle;
        }
    }
    let relative_mass_above = ABOVE_CLIFF
        .iter()
        .map(|offset| run(animal, high + offset, tempo, BODY_TIME).final_mass / healthy_mass)
        .collect();
    let collapse_time_below = run(animal, low - BELOW_CLIFF, tempo, BODY_TIME).collapsed_at;
    Measurement {
        tempo,
        cliff: Cliff::Between {
            collapses: low,
            survives: high,
        },
        non_monotone,
        healthy_mass,
        relative_mass_above,
        collapse_time_below,
    }
}

fn main() {
    let started = Instant::now();

    // 既存のテストと同じ条件(テンポ ×1、600 ステップ)で崖を再現できるか
    let orbium = load_animal("O2u").unwrap();
    for scale in [0.78f32, 0.77] {
        let outcome = run(&orbium, scale, 1.0, 600.0);
        println!(
            "再現確認 O2u growth_scale {scale:.2}、テンポ ×1、600 ステップ: 総量 {:.2}{}",
            outcome.final_mass,
            outcome
                .collapsed_at
                .map_or(String::new(), |t| format!("(体の時間 {t:.0} で崩壊)"))
        );
    }
    println!();

    let animals: Vec<(String, Animal)> = vmc_pet_body::list_animals()
        .unwrap()
        .into_iter()
        .map(|(code, _)| {
            let animal = load_animal(&code).unwrap();
            (code, animal)
        })
        .collect();
    let results: Vec<(String, Vec<Measurement>)> = thread::scope(|scope| {
        let handles: Vec<_> = animals
            .iter()
            .map(|(code, animal)| {
                scope.spawn(move || {
                    let measurements = TEMPOS.iter().map(|t| measure(animal, *t)).collect();
                    (code.clone(), measurements)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect()
    });

    println!(
        "体の時間 {BODY_TIME:.0} だけ進めたときの成長の強さの崖(テンポが小さいほど時間の刻みが細かい)"
    );
    println!(
        "  崖のすぐ上の総量: 崖の +{:?} での、同じテンポの健康な総量に対する割合",
        ABOVE_CLIFF
    );
    println!("  崩れる速さ: 崖の -{BELOW_CLIFF} で崩壊するまでの体の時間");
    for (code, measurements) in &results {
        println!("{code}");
        for m in measurements {
            let cliff = match m.cliff {
                Cliff::Between {
                    collapses,
                    survives,
                } => format!("{collapses:.4}〜{survives:.4}"),
                Cliff::NeverCollapses => "0.50 以上ではずっと生き延びる".to_string(),
                Cliff::AlwaysCollapses => "1.00 まで全部崩壊".to_string(),
            };
            let above: Vec<String> = m
                .relative_mass_above
                .iter()
                .map(|r| format!("{r:.2}"))
                .collect();
            println!(
                "  テンポ ×{:<4} 崖 {cliff:>14} | 崖のすぐ上の総量 [{}] | 崩れる速さ {} | 健康な総量 {:.1}{}",
                m.tempo,
                above.join(", "),
                m.collapse_time_below
                    .map_or("崩壊せず".to_string(), |t| format!("{t:.0}")),
                m.healthy_mass,
                if m.non_monotone {
                    " | 崖が1つでない"
                } else {
                    ""
                }
            );
        }
    }
    println!();
    println!("合計 {:.0} 秒", started.elapsed().as_secs_f32());
}
