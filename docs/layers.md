# 体・姿勢・操作の3層(値の流れと入口)

**この文書は地図である。** どの値がどこから来て、どこへ行くかだけを扱う。

- 設計の**判断**とその根拠 → `docs/DESIGN.md`
- 実験の**実測** → `docs/experiments/*.md`
- 姿勢層(ジェスチャー)そのもの → `docs/experiments/expression-layer.md`

層が増えたときに「この値をいじると何が変わるか」を引くための索引として使う。
確度は明示する(確認済み / たぶん / 未検証)。ここに書いてある数はすべて
コードを読んで数えたもので、実測は参照先にある。

## 1. 層の対応表

| 層 | 何を決めるか | 主な型 | 置き場所 |
|---|---|---|---|
| 体のパラメータ | 体が**どう動くか**(生成則) | `LeniaBody` / `ParticleBody`(+ `ParticleParams` / `Vitality` / `Mood`) | `crates/vmc-pet-body/src/{lenia,particles,particles_body,lenia_body,vitality,mood}.rs` |
| コントローラからの操作 | 体に**何を働きかけるか** | `AutonomousController`(Lenia)/ `ParticleBody::controller_act`(粒子) | `crates/vmc-pet-body/src/controller.rs` / `particles_body.rs` |
| 姿勢のパラメータ | 体を**どう見せるか** | `Pose` / `Gesture` / `Expression` / `ResolvedPose` | `crates/vmc-pet-body/src/gesture.rs` |
| 姿勢の描画 | 姿勢を**ドットにする** | `PoseProjection` / `draw_posed` | `src/render/pose.rs` / `src/render/dot_grid.rs` |

**体の境界**は `BodyPort`(`crates/vmc-pet-body/src/port.rs`)。外から体にできるのは
「摂動を注入する」と「場を読む」だけで、内部の更新規則に触る経路はない。
`LeniaBody`・`ParticleBody`・`Pet` の3つが実装している。

## 2. 値の流れ

```
気分・エネルギー (Mood / Vitality)
  ├─ tempo ────────→ 体の時間
  │                  Lenia: 1ステップで進む時間 / 粒子: 位置へ反映する割合
  ├─ 元気 ─────────→ Lenia: 成長の強さ(growth_scale)
  │                  粒子: 力全体の倍率(force_scale = 0.6 + 0.4 × growth_scale)
  └─ energy ───────┐
                   │
体 ──→ 場(32×32、`FieldView`)│
  ├─ cohesion(10セル) ───────┤
  ├─ コントローラの観測 ──────┤
  └─ 描画(dot_grid)          ↓
                     姿勢層 (Expression)
                       energy / cohesion / tempo / 放置秒数
                       → weights(8つ) → Pose::mix → resolve
                       → ResolvedPose → ドットグリッドの描画
```

**向きは一方向である。** 姿勢から体へ戻る経路は1本もない(確認済み)。姿勢は
「体が作った場をどう見せるか」だけを決める。

## 3. 姿勢パラメータへの入口は1本しかない

| 入口 | どこから | 体を粒子に変えると変わるか |
|---|---|---|
| 8ジェスチャーの**重み** | `weights(energy, cohesion, tempo, unattended_seconds)` | 下の3つは変わらず、`cohesion` だけ変わる |
| 位相の進み方 | `2π × pose.freq × seconds` | 混合の副産物 |
| 葛藤としての roll | `2*min(lift, sink) × DEG_PER_CONFLICT` | 混合の副産物(外部入力ではない) |

- 入力4つの出どころ: 元気 = `Vitality` / がっかり = `Mood` / 放置秒数 = `Expression` 自身 /
  まとまり = **体が作った場**
- つまり**体から来るのは「まとまり」1つだけ**。他3つは体が違っても同じ値になる
- 語彙(`GESTURES`)は `const` で、実行時に書き換える経路は**ゼロ**(確認済み)。
  重みは非負で合計1に正規化されるので、**出せる姿勢は8点の凸包の中だけ**
- `--pose <名前>` と `pin_gesture` は語彙の1点を**選ぶ**だけで、値を変えない

**同じ名前の「まとまり」が2つある**(混同しやすい):

| 型 | 何を測るか | 誰が使うか |
|---|---|---|
| `FieldView::cohesion(半径)` | 重心から半径内の**総量の割合** | **姿勢層** |
| `ParticleObservation.cohesion` | **最大の塊にいる粒子の割合** | 粒子の**コントローラ** |

## 4. コントローラからの操作(体ごとに排他)

| | Lenia | 粒子 |
|---|---|---|
| 観るもの | 場の歪み + 元気 | `ParticleObservation`(塊の割合・重心・広がり・はぐれ数) |
| 判断の間隔 | 17ステップごと | 毎ステップ |
| 行動 | 歪み > 0.2543 なら、重心から 4.2958 セルの位置を半径 4.7612・強さ 0.0239 で突く(元気が尽きていれば強さ ×0.25) | はぐれ ≥ 1 なら `lure = (最大の塊の重心, 0.13)`、届く距離は箱いっぱい |
| 注入の経路 | `Perturbation` → `LeniaBody::disturb` | 加速度の項(`world.lure`) |
| 持ち主 | **`Pet`**(`Pet::controller`) | **体の中**(`controller_act`) |

**どちらも `disturb` 相当で、エネルギーは変えない**(規律)。人間のクリックだけが
両方に共通し、`disturb`(体)と `receive_care`(元気)の**2経路に分かれる**。

> **既知の非対称**(記録。破綻はしていない): コントローラの持ち主が違う。Lenia のは
> `Pet` が持ち、粒子のは体の中にある。3体目を足すときに再検討する。

## 5. 単位の変換点

系が3つある。**セル**(物理)・**場の値**(0..1)・**表示ドット + 姿勢の度/pose unit**。

| 変換 | 値 | どこ |
|---|---|---|
| 物理セル → 表示ドット | `ZOOM = 2`(`BOX_SIZE 16` → `VIEW_SIZE 32`) | `particles_body.rs` |
| 粒子1個 → 場の値 | `PARTICLE_WEIGHT = 0.35`(1マス 1.0 で飽和) | `particles_body.rs` |
| クリックの量 → 突きの速さ | `POKE_IMPULSE_PER_AMOUNT = 5.0` | `particles_body.rs` |
| 姿勢の長さ(pose unit) → セル | `CELLS_PER_POSE_UNIT = 2.5` | `gesture.rs` |
| 葛藤 → roll の度 | `DEG_PER_CONFLICT = 60` | `gesture.rs` |
| 元気 → 力の倍率 | `force_scale = 0.6 + 0.4 × growth_scale` | `particles_body.rs` |

**角度(度)は変換しない。** 姿勢の pitch / yaw / roll はそのまま描画へ渡る。

## 6. 増えてよい複雑さ / 危ない複雑さ

**増えてよい方** — 「型の分岐」: `impl Body`(enum)の機械的な委譲。**11メソッド**が
1箇所にまとまっている(`pet.rs`)。体を足すときは、ここに1腕を足すだけでよい。

**増やしてはいけない方** — 「`Pet` の本体に散った体ごとの判断」。現在は**3種類**:

1. 構築(`with_controller_params` / `with_particle_body`)
2. `Pet::step` の `match`(コントローラと自己行動の出し方)
3. `Pet::step` の echo・色素の重心ゲート(`is_particle_body`)

`inject` の分岐は 2026-10 に畳んだ(issue #19)。**注入の意味は体ごとに違う**
(Lenia は突き + 世話、粒子は突きだけ)ので、その判断は各体の `BodyPort::inject` に
戻してあり、`Pet` は委譲するだけである。

**増分の目安**: 新しい体を足すと、`impl Body` に1腕(機械的)+ 上の3種類のうち
該当するもの、が増える(3〜5箇所。**推定・未検証** — 実際に3体目を足して数えた人は
まだいない)。

**崩壊検知の戻り値は体で濃さが違う**: `LeniaBody::step()` は本当に崩壊を検知して
`true` を返しうるが、`ParticleBody::step()` は常に `false`(粒子は数が保存される)。

## 7. 確かめていないこと

1. **粒子の体で姿勢層を動かしていない。** `--animal particles:1091` と `--pose` は
   フラグ上は併用できるが、粒子の体は表示が原点固定の zoom 2 なので、傾きの軸と
   板の厚み(`SLAB_THICKNESS_CELLS = 3.5`)の見え方は測り直しが要る
2. **粒子の体の「まとまり」が Lenia と同じように 0.93〜1.00 に張り付くか。**
   張り付かなければ、粒子の体では `shake` / `lean_in` / `flicker` が立ち、
   同じペットでも姿勢の出方が変わる
3. **コントローラの行動と姿勢の因果。** 「コントローラが整える → まとまりが上がる →
   `nod` が出る」という間接経路しかないが、実際に見えるかは測っていない
4. **`flicker` は実運用でほぼ立たない**(まとまりが 0.93〜1.00 に張り付くため)。
   `flicker` の重みは `4·energy(1−energy)·(1−cohesion)` に比例するので、
   まとまりが高いとゼロになる
5. **姿勢の沈み込み(`offset_cells`)と粒子の箱の壁**(`WALL_MARGIN = 1.5`)の干渉。
   沈んだ絵が箱の外へ出る見え方は未検証
