//! 自律的に働きかけるコントローラ(C)。人間のタッチとは独立に、体自身の形
//! (重心から見た質量の偏り)を観測し、それをならす向きへ軽く自己摂動する。
//!
//! World Models (Ha & Schmidhuber, 2018) の M(世界モデル)/C(コントローラ)の
//! 分割で言う C にあたる。「体(M)を観測してから行動を決める」という構造だけを
//! 借りていて、学習は一切していない――手で決めた閾値・向き・強さで、偏りをならす
//! 向きに軽く注入するだけの単純な規則(docs/DESIGN.md参照)。
//!
//! 自己摂動は `LeniaBody::disturb`(エネルギーを変えない経路)を通す。人間の
//! タッチが常にエネルギーを回復させるのは「世話をされた」ことの表れであり、
//! コントローラの自己摂動まで同じ扱いにすると、自分で自分を回復させ続けられて
//! しまい、「放置されると弱る」という設計の前提が壊れるためである。

use crate::{CellPos, FieldView, Perturbation};

/// コントローラが行動を決めるときに見るもの。
///
/// 当初は場(`FieldView`)だけを渡していた。そのためコントローラは「どう扱われて
/// きたか」を知らず、世話のされ方が振る舞いに出るのは間接的な物理
/// (エネルギー → growth_scale → 場の力学)経由だけだった。
/// 読み取りやすさ(`fitness::legibility`)を上げるには、状態に応じて振る舞いを
/// **変える**必要があるため、内部状態もここから渡す。
///
/// 位置引数を増やすのではなく構造体にしてあるのは、ロードマップ上この観測を
/// さらに広げていく前提があるためと、`maybe_act(field, 0.3)` のような呼び出しが
/// 何を渡しているのか読めなくなるのを避けるため。
#[derive(Debug, Clone, Copy)]
pub struct Observation<'a> {
    /// 体の場。形の歪みを見るのに使う。
    pub field: FieldView<'a>,
    /// 気分状態(0.0..=1.0)。世話され続けていれば高く、放置されると尽きる。
    pub energy: f32,
    /// いま世話が来ることをどれだけ期待しているか(0.0..=1.0)。
    /// 経験から学んだ生活リズムによる(`CarePredictor::anticipation_at`)。
    /// 時計を持たない場面(評価・テスト)では 0.0。
    pub anticipation: f32,
}

/// コントローラの挙動を決める定数一式。
///
/// 元々はモジュール内の定数として直接埋め込んでいたが、パラメータ探索
/// (`crates/vmc-pet-body/examples/search_controller_params.rs`)や将来の
/// 学習可能なコントローラが「同じ規則を、違う定数で試す」ことを必要とするため、
/// 実行時に差し替えられる構造体に切り出した。
///
/// `Default` は、ランダムサーチ300通りで見つかった値
/// (`examples/search_controller_params.rs`、docs/DESIGN.md
/// 「パラメータ探索を試した」参照)。手で決めていた値
/// (30ステップに1度・閾値0.3・半径4.0・強さ0.10・距離3.0)から、
/// より頻繁に(17ステップに1度)・より弱く(0.024)・より遠くへ(4.30セル)
/// 働きかける組み合わせに変わった。
///
/// その後、世代交代による探索でより動きの大きい候補
/// (10ステップに1度・強さ0.156)が見つかり、長時間・全生物・近傍の検証を
/// 通ったため採用を試みたが、**採用を取り消した**。ソースへ書き下す際に値を
/// 丸めた(1e-5程度)だけで、20000ステップ中に崩壊するようになったためである。
/// Lenia はカオス系で、1e-5 の差でも2万ステップかけて軌跡が発散するため、
/// 「長時間1回走らせて無事だった」は書き下せる精度のパラメータの性質として
/// 成立していなかった。この強さの領域では、22分のセッション中に一度は
/// 崩壊(安全装置で置き直されるが、画面上は元の姿へ突然戻る)が起きる確率が
/// 無視できない。動きの見やすさより、そうならないことを優先して現在の
/// 弱い値に留めてある(docs/DESIGN.md「丸めで崩壊が反転した」参照)。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ControllerParams {
    /// 何ステップに一度、形を観測して判断するか。
    pub evaluate_every_steps: u32,
    /// 体の歪み(`imbalance` が返す歪度の大きさ)が、これを超えたらならす方向へ
    /// 軽く注入する。実測(O2u を300ステップ動かして10ステップごとに観測)では
    /// 歪度の大きさが常に 0.6〜0.7 程度で安定していたため、それより十分低い
    /// 閾値にして、健常な体では確実に反応するようにしてある。
    pub imbalance_threshold: f32,
    /// 自己摂動の半径(セル)。
    pub nudge_radius_cells: f32,
    /// 自己摂動の強さ。`touch.rs` の `CLICK_BODY_AMOUNT`(0.20)より弱くしてある。
    /// クリックは一度きりだが、こちらは条件を満たすたびに繰り返し働きかけるため、
    /// 1回あたりは控えめにする必要がある。
    pub nudge_amount: f32,
    /// 重心から自己摂動の位置までの距離(セル)。
    pub nudge_offset_cells: f32,
    /// エネルギー(気分)が尽きているときに、自己摂動の強さを何倍にするか。
    ///
    /// 1.0 なら状態に関係なく同じ強さ(観測にエネルギーを加える前の挙動)、
    /// 0.0 なら尽きたときは何もしない。元気なときほど活発に、弱ると静かに
    /// なるので、世話のされ方が見て分かるようになる
    /// (`fitness::legibility` で測る)。
    ///
    /// 当初はこれを安全のための制約とも考えていた(弱った体を強く揺らすと
    /// 自己修復できずに崖へ落ちるのでは、という懸念)。世話の予測を振る舞いに
    /// 繋ぐ際、弱った体を常に全力で揺らす設定(この値を 1.0)で全生物を
    /// 20000ステップ動かし、揺らす強さを±5%ずらした近傍も含めて測ったところ、
    /// 崩壊は1件も起きなかった。したがって、この値は安全の根拠ではなく、
    /// **見た目の読み取りやすさのためのもの**である。安全は `nudge_amount`
    /// 自体を控えめにしていることで保たれている。
    ///
    /// 世話を期待している時間は、エネルギーが尽きていてもこの倍率を超えて
    /// 満タン時の強さまで引き上げられる(`Observation::anticipation`)。
    pub nudge_amount_when_depleted: f32,
}

impl Default for ControllerParams {
    fn default() -> Self {
        Self {
            evaluate_every_steps: 17,
            imbalance_threshold: 0.2543,
            nudge_radius_cells: 4.7612,
            nudge_amount: 0.0239,
            nudge_offset_cells: 4.2958,
            nudge_amount_when_depleted: 0.25,
        }
    }
}

/// 体の形を観測し、周期的に自己摂動でならす、単純な規則ベースのコントローラ。
pub struct AutonomousController {
    params: ControllerParams,
    steps_since_last_evaluation: u32,
}

impl AutonomousController {
    /// 現在の採用値(`ControllerParams::default()`)で作る。
    pub fn new() -> Self {
        Self::with_params(ControllerParams::default())
    }

    /// パラメータ探索・学習など、既定値以外を試したいときに使う。
    pub fn with_params(params: ControllerParams) -> Self {
        Self {
            params,
            steps_since_last_evaluation: 0,
        }
    }

    /// 体が1ステップ進むたびに呼ぶ。判断のタイミングでないか、偏りが閾値未満
    /// なら `None`(何もしない)。
    pub fn maybe_act(&mut self, observation: Observation<'_>) -> Option<Perturbation> {
        let Observation {
            field,
            energy,
            anticipation,
        } = observation;
        self.steps_since_last_evaluation += 1;
        if self.steps_since_last_evaluation < self.params.evaluate_every_steps {
            return None;
        }
        self.steps_since_last_evaluation = 0;

        let (centroid_x, centroid_y) = field.toroidal_centroid()?;
        let (imbalance_x, imbalance_y) = imbalance(field, centroid_x, centroid_y);
        let magnitude = crate::math::sqrtf(imbalance_x * imbalance_x + imbalance_y * imbalance_y);
        if magnitude < self.params.imbalance_threshold {
            return None;
        }

        // 偏っている向き(重い側)に沿って、重心から少し離れた位置へ軽く注入し、
        // 軽い側を持ち上げてならす。
        let (direction_x, direction_y) = (imbalance_x / magnitude, imbalance_y / magnitude);
        let width = field.width() as f32;
        let height = field.height() as f32;
        let target_x = crate::math::rem_euclidf(
            centroid_x + direction_x * self.params.nudge_offset_cells,
            width,
        );
        let target_y = crate::math::rem_euclidf(
            centroid_y + direction_y * self.params.nudge_offset_cells,
            height,
        );

        // 元気なほど強く、弱るほど控えめに揺らす。ただし世話が来そうな時間は、
        // 弱っていても元気なときと同じ強さまで活発になる(先回り)。
        // 期待が引き上げられるのは満タン時の強さまでで、それを超えることはない。
        // 弱った体を常に全力で揺らしても全生物が崩壊しないことは計測済み
        // (docs/DESIGN.md「世話の予測を振る舞いに繋ぐ」)。
        let liveliness = energy.clamp(0.0, 1.0).max(anticipation.clamp(0.0, 1.0));
        let vigour = self.params.nudge_amount_when_depleted
            + (1.0 - self.params.nudge_amount_when_depleted) * liveliness;

        Some(Perturbation {
            at: CellPos {
                x: target_x as usize,
                y: target_y as usize,
            },
            radius: self.params.nudge_radius_cells,
            amount: self.params.nudge_amount * vigour,
        })
    }
}

impl Default for AutonomousController {
    fn default() -> Self {
        Self::new()
    }
}

/// 重心から見た形の歪み(統計でいう歪度・skewness)を `(x方向, y方向)` で返す。
/// 正なら重心より大きい座標側に長く伸びている。左右対称なら 0 になる。
///
/// 「重心から見て軽い側・重い側」を単純に符号(+1/-1)で数える案も試したが、
/// 重心自体がその定義上ほぼ釣り合う点であるため信号が弱く、しかも符号は
/// 不連続な階段関数なので、重心の計算(トーラス上の円周平均、浮動小数点の
/// 誤差を含む)がほんの少しずれただけで正負が丸ごと反転してしまい、実際に
/// 「ある行にしか質量が無い(その軸ではまったく対称)」場ですら誤って強い
/// 偏りを検出する不具合を起こした。歪度(3次モーメント)は連続的な量なので、
/// 重心の微小なずれに対して滑らかにしか変化せず、この不具合を起こさない。
fn imbalance(field: FieldView<'_>, centroid_x: f32, centroid_y: f32) -> (f32, f32) {
    let width = field.width() as f32;
    let height = field.height() as f32;
    let mut mass = 0.0;
    let mut variance_x = 0.0;
    let mut variance_y = 0.0;
    let mut third_moment_x = 0.0;
    let mut third_moment_y = 0.0;

    for y in 0..field.height() {
        for x in 0..field.width() {
            let value = field.get(x, y);
            if value <= 0.0 {
                continue;
            }
            mass += value;
            let dx = crate::field::toroidal_signed_offset(x as f32 - centroid_x, width);
            let dy = crate::field::toroidal_signed_offset(y as f32 - centroid_y, height);
            variance_x += value * dx * dx;
            variance_y += value * dy * dy;
            third_moment_x += value * dx * dx * dx;
            third_moment_y += value * dy * dy * dy;
        }
    }

    if mass <= 0.0 {
        return (0.0, 0.0);
    }
    (
        skewness(third_moment_x / mass, variance_x / mass),
        skewness(third_moment_y / mass, variance_y / mass),
    )
}

/// 標準化された歪度: 3次モーメントを標準偏差の3乗で割る。
/// 広がりがほぼ無い(標準偏差がほぼ0)場合は歪度を定義できないため 0 を返す。
fn skewness(third_moment: f32, variance: f32) -> f32 {
    if variance <= 1e-6 {
        return 0.0;
    }
    third_moment / (variance * crate::math::sqrtf(variance))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Field;

    /// 満タンのエネルギーで場を観測する。強さへの影響を見たいテストだけが
    /// エネルギーを変える。
    fn observing(field: &Field) -> Observation<'_> {
        Observation {
            field: field.view(),
            energy: 1.0,
            anticipation: 0.0,
        }
    }

    /// 重さの違う2点(本体・尾に見立てる)だけを持つ、はっきり歪んだ場を作る。
    ///
    /// 単純な矩形(一様に埋めた四角)は使わない: 一様な塊は対称なので、
    /// どこに置いても歪度は(定義上正しく)0になる。値の異なる2点を使うことで、
    /// 確実に非対称な形を作れる。
    ///
    /// 2点は場の半分近くを隔てるほど離してはいけない。円周上の平均である
    /// `toroidal_centroid` は、質量が円周のほぼ反対側に分かれるほど、単純な
    /// 算術平均とかけ離れた(直感に反する)点を返しうる。
    fn lopsided_field(width: usize, height: usize) -> Field {
        let mut field = Field::new(width, height);
        let (heavy_x, light_x) = (width / 3, width / 3 + width / 4);
        let y = height / 2;
        field.map(|x, cell_y, _value| {
            if cell_y != y {
                0.0
            } else if x == heavy_x {
                1.0
            } else if x == light_x {
                0.3
            } else {
                0.0
            }
        });
        field
    }

    #[test]
    fn a_two_point_field_has_a_strong_skew() {
        // Arrange: 重い点(本体)と軽い点(尾)がある
        let field = lopsided_field(32, 32);
        let centroid = field.view().toroidal_centroid().unwrap();

        // Act
        let (imbalance_x, imbalance_y) = imbalance(field.view(), centroid.0, centroid.1);

        // Assert: x方向にはっきり歪んでいるはず。同じ行にしかないので y は歪まない
        assert!(
            imbalance_x.abs() > 0.3,
            "expected a strong x skew, got {imbalance_x}"
        );
        assert!(
            imbalance_y.abs() < 0.01,
            "y must stay symmetric, got {imbalance_y}"
        );
    }

    #[test]
    fn a_symmetric_field_has_no_imbalance() {
        // Arrange: 中央に対称な塊
        let mut field = Field::new(32, 32);
        field.map(|x, y, _value| {
            if (12..20).contains(&x) && (12..20).contains(&y) {
                1.0
            } else {
                0.0
            }
        });

        // Act
        let centroid = field.view().toroidal_centroid().unwrap();
        let (imbalance_x, imbalance_y) = imbalance(field.view(), centroid.0, centroid.1);

        // Assert
        assert!(imbalance_x.abs() < 0.01, "got {imbalance_x}");
        assert!(imbalance_y.abs() < 0.01, "got {imbalance_y}");
    }

    #[test]
    fn a_balanced_field_never_acts() {
        // Arrange: 対称な場は、何ステップ判定を進めても偏りが閾値を超えない
        let field = {
            let mut f = Field::new(32, 32);
            f.map(|x, y, _value| {
                if (12..20).contains(&x) && (12..20).contains(&y) {
                    1.0
                } else {
                    0.0
                }
            });
            f
        };
        let mut controller = AutonomousController::new();

        // Act / Assert
        for _ in 0..(ControllerParams::default().evaluate_every_steps * 3) {
            assert!(controller.maybe_act(observing(&field)).is_none());
        }
    }

    #[test]
    fn a_lopsided_field_is_nudged_toward_the_lighter_side() {
        // Arrange
        let field = lopsided_field(32, 32);
        let mut controller = AutonomousController::new();

        // Act: 判定のタイミングまで進める
        let mut perturbation = None;
        for _ in 0..ControllerParams::default().evaluate_every_steps {
            perturbation = controller.maybe_act(observing(&field));
        }

        // Assert: 何かに向けて軽く注入する
        let perturbation = perturbation.expect("a strongly lopsided field must trigger a nudge");
        assert!(perturbation.amount > 0.0);
    }

    /// 指定したエネルギーと期待で、1回ぶんの自己摂動を取り出す。
    fn nudge_when(field: &Field, energy: f32, anticipation: f32) -> Perturbation {
        let mut controller = AutonomousController::new();
        let mut perturbation = None;
        for _ in 0..ControllerParams::default().evaluate_every_steps {
            perturbation = controller.maybe_act(Observation {
                field: field.view(),
                energy,
                anticipation,
            });
        }
        perturbation.expect("a strongly lopsided field must trigger a nudge")
    }

    /// 指定したエネルギーで(世話を期待していないときの)1回ぶんの自己摂動を取り出す。
    fn nudge_at_energy(field: &Field, energy: f32) -> Perturbation {
        nudge_when(field, energy, 0.0)
    }

    #[test]
    fn a_weakened_pet_stirs_itself_more_gently() {
        // Arrange
        let field = lopsided_field(32, 32);

        // Act
        let lively = nudge_at_energy(&field, 1.0);
        let weary = nudge_at_energy(&field, 0.0);

        // Assert: 弱っているほど控えめに揺らす。これが世話のされ方を
        // 見て分かるようにする唯一の経路(fitness::legibility で測る)
        assert!(
            weary.amount < lively.amount,
            "a weakened pet must stir itself more gently; lively={} weary={}",
            lively.amount,
            weary.amount
        );
        // 尽きても完全に止まりはしない(既定では満タン時の4分の1)
        assert!(weary.amount > 0.0);
        // 位置と広がりは状態に依らない
        assert_eq!(weary.at, lively.at);
        assert_eq!(weary.radius, lively.radius);
    }

    #[test]
    fn a_weakened_pet_that_expects_care_stirs_as_if_it_were_lively() {
        // Arrange
        let field = lopsided_field(32, 32);

        // Act
        let lively = nudge_when(&field, 1.0, 0.0);
        let weary_but_expecting = nudge_when(&field, 0.0, 1.0);
        let weary_half_expecting = nudge_when(&field, 0.0, 0.5);
        let weary = nudge_when(&field, 0.0, 0.0);

        // Assert: 世話が来そうな時間は弱っていても元気なときと同じ強さになり、
        // 期待の度合いに応じてその間を動く
        assert_eq!(weary_but_expecting.amount, lively.amount);
        assert!(weary.amount < weary_half_expecting.amount);
        assert!(weary_half_expecting.amount < weary_but_expecting.amount);
    }

    #[test]
    fn expecting_care_never_stirs_harder_than_a_lively_pet() {
        // Arrange
        let field = lopsided_field(32, 32);

        // Act: 元気なうえに期待もしている
        let lively = nudge_when(&field, 1.0, 0.0);
        let lively_and_expecting = nudge_when(&field, 1.0, 1.0);

        // Assert: 期待は満タン時の強さを超えさせない(検証済みの範囲に留める)
        assert_eq!(lively_and_expecting.amount, lively.amount);
    }

    #[test]
    fn does_not_evaluate_before_its_interval_elapses() {
        // Arrange
        let field = lopsided_field(32, 32);
        let mut controller = AutonomousController::new();

        // Act / Assert: 間隔に満たない間は何もしない
        for _ in 0..(ControllerParams::default().evaluate_every_steps - 1) {
            assert!(controller.maybe_act(observing(&field)).is_none());
        }
    }
}
