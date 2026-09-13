//! 自律コントローラのパラメータ(`ControllerParams`)を、`fitness::Trajectory`
//! を使って探索する。docs/DESIGN.md「学習可能なコントローラへ向けた代理指標」
//! および「パラメータ探索を試した」参照。
//!
//! **旧い道具**: 探索の目的関数(`fitness` = 動きの多さ)は、目的関数としては採らないと
//! 決めたもの。この指標を最大化する方向には出荷できる精度で安全な設定が無く
//! (docs/DESIGN.md「丸めで崩壊が反転した(単発の長時間実行は安全の証拠として弱い)」)、
//! 経験に関わる要素も入っていなかった(「世話のされ方が振る舞いに現れているか(legibility)」)。
//! 判断の記録にある探索を再現するために残してある。3段の安全性のゲート(短い窓・
//! 長い窓・全生物と近傍)の組み方は、今後パラメータを探すときにも参考になる。
//!
//! 探索は3つの段で構成される。段を分けているのは、安さと確かさが逆になって
//! いるためで、安い評価で候補を絞り、高い評価で確かめる:
//!
//! 1. **短い窓(900ステップ ≈ 60秒、O2u のみ)** — 動きの多さを測る。安い。
//!    ただし実際に崩壊するかどうかの予兆にはならないことが分かっている
//!    (fitness.rs 参照)。実際に、短い窓で1位だった候補が長い窓で崩壊した
//! 2. **長い窓(20000ステップ ≈ 22分、O2u)** — 崩壊しないことを確かめる
//! 3. **全生物の長い窓** — コントローラは全生物に出荷されるため、O2u だけで
//!    検証した値を採用すると別の生物を壊しうる。クリックと違ってコントローラは
//!    勝手に発火するので、ユーザーが何もしていないのに体が壊れることになる
//!
//! 世代交代は素朴な (μ+λ): 各世代で上位 μ 個を残し、その周辺を揺らした子を
//! λ 個作る。CMA-ES のような本格的な進化戦略ではないが、外部クレートに一切
//! 頼らず、何をしているか読んで追えることを優先した。

use vmc_pet_body::fitness::{Trajectory, COLLAPSE_PENALTY};
use vmc_pet_body::{ControllerParams, Pet};

const FIELD_SIZE: usize = 32;
const WARMUP_STEPS: u32 = 60;
const SHORT_EVAL_STEPS: u32 = 900;
const LONG_EVAL_STEPS: u32 = 20_000;

/// 世代数と、各世代で残す親の数・作る子の数。
const GENERATIONS: usize = 12;
const PARENTS: usize = 4;
const CHILDREN: usize = 24;

/// 最後に長い窓で確かめる候補の数。
const FINALISTS: usize = 3;

/// 外部クレートに頼らない、この探索専用の決定的な疑似乱数(xorshift64)。
/// 固定シードにしてあるので、実行するたびに同じ探索をやり直せる。
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

    /// -1.0..=1.0 のゆらぎ。変異に使う。
    fn jitter(&mut self) -> f32 {
        self.range_f32(-1.0, 1.0)
    }
}

/// 各パラメータの探索範囲。現在の採用値を含み、明らかに極端すぎる値
/// (反応が無くなる・逆に暴れすぎる)は除いてある。
///
/// `nudge_amount_when_depleted` はここに入れていない。この探索の目的関数は
/// 「動きの多さ」(`Trajectory::fitness`)で、弱っているときに**動きを抑える**
/// ことの価値を測れない。入れれば必ず 1.0(=エネルギーに依らず全力)へ押し
/// やられ、この値の存在意義(世話のされ方を見て分かるようにする)を消してしまう。
/// この値は `fitness::legibility` で別に評価する。
struct Bounds {
    evaluate_every_steps: (u32, u32),
    imbalance_threshold: (f32, f32),
    nudge_radius_cells: (f32, f32),
    nudge_amount: (f32, f32),
    nudge_offset_cells: (f32, f32),
}

const BOUNDS: Bounds = Bounds {
    evaluate_every_steps: (10, 60),
    imbalance_threshold: (0.15, 0.5),
    nudge_radius_cells: (2.0, 6.0),
    nudge_amount: (0.02, 0.30),
    nudge_offset_cells: (1.0, 6.0),
};

fn random_params(rng: &mut Rng) -> ControllerParams {
    ControllerParams {
        evaluate_every_steps: rng
            .range_u32(BOUNDS.evaluate_every_steps.0, BOUNDS.evaluate_every_steps.1),
        imbalance_threshold: rng
            .range_f32(BOUNDS.imbalance_threshold.0, BOUNDS.imbalance_threshold.1),
        nudge_radius_cells: rng.range_f32(BOUNDS.nudge_radius_cells.0, BOUNDS.nudge_radius_cells.1),
        nudge_amount: rng.range_f32(BOUNDS.nudge_amount.0, BOUNDS.nudge_amount.1),
        nudge_offset_cells: rng.range_f32(BOUNDS.nudge_offset_cells.0, BOUNDS.nudge_offset_cells.1),
        ..ControllerParams::default()
    }
}

/// 親の周辺を揺らした子を作る。揺らす幅は各パラメータの範囲の `spread` 割。
fn mutate(parent: &ControllerParams, rng: &mut Rng, spread: f32) -> ControllerParams {
    let jitter_within = |rng: &mut Rng, value: f32, (low, high): (f32, f32)| {
        (value + rng.jitter() * (high - low) * spread).clamp(low, high)
    };
    let steps_span =
        (BOUNDS.evaluate_every_steps.1 - BOUNDS.evaluate_every_steps.0) as f32 * spread;
    let steps = (parent.evaluate_every_steps as f32 + rng.jitter() * steps_span)
        .round()
        .clamp(
            BOUNDS.evaluate_every_steps.0 as f32,
            BOUNDS.evaluate_every_steps.1 as f32,
        ) as u32;

    ControllerParams {
        evaluate_every_steps: steps,
        imbalance_threshold: jitter_within(
            rng,
            parent.imbalance_threshold,
            BOUNDS.imbalance_threshold,
        ),
        nudge_radius_cells: jitter_within(
            rng,
            parent.nudge_radius_cells,
            BOUNDS.nudge_radius_cells,
        ),
        nudge_amount: jitter_within(rng, parent.nudge_amount, BOUNDS.nudge_amount),
        nudge_offset_cells: jitter_within(
            rng,
            parent.nudge_offset_cells,
            BOUNDS.nudge_offset_cells,
        ),
        ..*parent
    }
}

/// ソースへ書き下すときの精度(小数4桁)に丸める。
///
/// これを入れたのは、探索で見つけた候補をそのまま採用しようとして踏んだ
/// 失敗から。厳密な値(0.32459584 など)は20000ステップ生き延びたのに、
/// ソースに書くとき小数4桁へ丸めた値(0.3246)では崩壊した。Lenia はカオス系
/// なので、1e-5 の差でも2万ステップかけて軌跡が発散し、崩壊するかどうかが
/// 反転する。**評価するのは、実際に出荷する値でなければ意味がない。**
fn as_written_in_source(params: ControllerParams) -> ControllerParams {
    let round4 = |value: f32| (value * 10_000.0).round() / 10_000.0;
    ControllerParams {
        evaluate_every_steps: params.evaluate_every_steps,
        imbalance_threshold: round4(params.imbalance_threshold),
        nudge_radius_cells: round4(params.nudge_radius_cells),
        nudge_amount: round4(params.nudge_amount),
        nudge_offset_cells: round4(params.nudge_offset_cells),
        nudge_amount_when_depleted: round4(params.nudge_amount_when_depleted),
    }
}

fn evaluate(code: &str, params: ControllerParams, steps: u32) -> f32 {
    let animal = vmc_pet_body::load_animal(code).unwrap();
    let mut pet = Pet::with_controller_params(animal, FIELD_SIZE, FIELD_SIZE, params);
    for _ in 0..WARMUP_STEPS {
        pet.step();
    }
    Trajectory::record(&mut pet, steps).fitness(FIELD_SIZE, FIELD_SIZE)
}

/// 全生物で長い窓を回し、1つでも崩壊したらその生物のコードを返す。
fn animal_that_collapses(params: ControllerParams) -> Option<String> {
    for (code, _name) in vmc_pet_body::list_animals().unwrap() {
        if evaluate(&code, params, LONG_EVAL_STEPS) == COLLAPSE_PENALTY {
            return Some(code);
        }
    }
    None
}

/// 近傍を揺らした候補のうち、何個が崩壊せずに済むかを数える。
///
/// 実測で、スコアがほとんど同じ(0.6091 / 0.6090 / 0.6088)でパラメータも
/// よく似た3候補のうち、1つだけが生き延びて2つは崩壊した。さらに、生き延びた
/// 候補もソースに書く精度へ丸めた途端に崩壊した。つまりこの領域では
/// 「崩壊するかどうか」は、ほぼ確率的な事象として振る舞う(20000ステップの
/// 間に一度でも崖に触れるか)。1回の長時間実行で無事だったことは、その
/// パラメータが安全である証拠として弱い。
///
/// そこで近傍を多めに叩いて、**崩壊の起こりにくさ**として見る。ここを
/// 通らない候補は、点として無事でも採用しない
/// (このプロジェクトが一貫して採ってきた「崖から十分離す」方針の延長)。
fn neighbourhood_survivors(params: ControllerParams, rng: &mut Rng) -> (u32, u32) {
    const PROBES: u32 = 12;
    const PROBE_SPREAD: f32 = 0.05;
    let mut survivors = 0;
    for _ in 0..PROBES {
        let probe = as_written_in_source(mutate(&params, rng, PROBE_SPREAD));
        if evaluate("O2u", probe, LONG_EVAL_STEPS) != COLLAPSE_PENALTY {
            survivors += 1;
        }
    }
    (survivors, PROBES)
}

fn main() {
    let default = ControllerParams::default();
    println!(
        "baseline (current defaults): short={:.4} long={:.4} {default:?}",
        evaluate("O2u", default, SHORT_EVAL_STEPS),
        evaluate("O2u", default, LONG_EVAL_STEPS),
    );
    println!();

    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    // 第1世代は範囲全体からの無作為。以降は上位の周辺を揺らす。
    let mut population: Vec<ControllerParams> =
        (0..CHILDREN).map(|_| random_params(&mut rng)).collect();
    // 手で決めた値も第1世代に入れておく(それより悪くなっていないかが見える)。
    population.push(default);

    let mut ranked: Vec<(ControllerParams, f32)> = Vec::new();
    for generation in 0..GENERATIONS {
        ranked = population
            .iter()
            .map(|params| (*params, evaluate("O2u", *params, SHORT_EVAL_STEPS)))
            .collect();
        ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        ranked.truncate(PARENTS);

        println!(
            "generation {generation:2}: best short-window score = {:.4}",
            ranked[0].1
        );

        // 次の世代: 親そのもの + 親の周辺を揺らした子。世代が進むほど揺らす幅を
        // 狭めて、粗く探してから細かく詰める。
        let spread = 0.30 * (1.0 - generation as f32 / GENERATIONS as f32) + 0.05;
        population = ranked.iter().map(|(params, _)| *params).collect();
        for index in 0..CHILDREN {
            let parent = ranked[index % ranked.len()].0;
            population.push(mutate(&parent, &mut rng, spread));
        }
    }

    println!();
    println!("verifying the top {FINALISTS}: long window -> every animal -> neighbourhood");
    println!("(候補は、実際にソースへ書く精度=小数4桁へ丸めてから検証する)");
    for (params, short_score) in ranked.iter().take(FINALISTS) {
        let params = &as_written_in_source(*params);
        let long_score = evaluate("O2u", *params, LONG_EVAL_STEPS);
        if long_score == COLLAPSE_PENALTY {
            println!("  short={short_score:.4} [COLLAPSED on O2u] {params:?}");
            continue;
        }
        if let Some(code) = animal_that_collapses(*params) {
            println!(
                "  short={short_score:.4} long={long_score:.4} [COLLAPSED on {code}] {params:?}"
            );
            continue;
        }
        let (survivors, probes) = neighbourhood_survivors(*params, &mut rng);
        println!(
            "  short={short_score:.4} long={long_score:.4} \
             [safe for every animal; neighbourhood {survivors}/{probes} survive] {params:?}"
        );
    }

    println!();
    println!("同じ検証を、いまの採用値についても行う(比較のため):");
    let (survivors, probes) = neighbourhood_survivors(default, &mut rng);
    match animal_that_collapses(default) {
        Some(code) => println!("  current defaults [COLLAPSED on {code}]"),
        None => println!(
            "  current defaults [safe for every animal; \
             neighbourhood {survivors}/{probes} survive]"
        ),
    }
}
