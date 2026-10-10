# 決定モデルの出力を CSS アニメーションのジェスチャーにする

decision model（System One API）に「どのジェスチャーを・どの強さで」を選ばせ、
その出力を CSS アニメーションのパラメータに写して、言葉を使わずに反応させる試み。

## 問いと背景

vmc-pet の表現軸は 色(色素) / テンポ / 身じろぎ の3つしかなく、「生き残る関数で表情を足せるか」
「別の形から戻る帰り道はあるか」の2つの実験で、**質の違う見た目は作れるが一方通行で、表情には
使えない**ことが分かっている（[expression-axes.md](expression-axes.md)）。これは Lenia 場という
体に起因する制約で、場は「安定に作れて、かつ可逆に戻れる」範囲しか表現できない。

CSS の `transform` / `opacity` / `filter` にはこの制約が無い。本質的に可逆で、崩壊の概念が無い。
そこで、**体の代わりに「ジェスチャーの語彙」を持ち、決定モデルに選択させる**路線を試した。

決定モデルはテキストを生成しない（`output_modalities = ["decisions"]`、全応答で
`output_tokens = 0`、`/v1/chat/completions` は HTTP 500）。したがって:
**ジェスチャーの定義（CSS）は手書きで固定し、決定モデルには選択と強度だけを担わせる。**
生成は決定論的な写像が行う。

## 方法・条件

- 決定モデル: `ggml-org/Laya-GGUF` Q8_0（0.42B）と `ggml-org/Clef-Flash-GGUF` Q4_K_M（9.08B）
- 実行: moon の CPU、`llama.cpp b11539`（`~/.local/share/llama.cpp/llama-b11539/llama-server`）
- ジェスチャー語彙 **8個**（手書き、`gestures.css`）: `idle` / `perk_up` / `nod` / `shake` /
  `lean_in` / `recoil` / `settle` / `flicker`
- 質問4つを1リクエストで:
  - `gesture` = `choice`（8候補）— どのジェスチャーか
  - `energy` = `score`（5段階）— 反応に使えるエネルギー
  - `approach` = `score`（5段階）— 利用者に引き寄せられるか遠ざかるか
  - `emit` = `noul` — そもそも反応するか
- 入力は日本語の短い状態記述 **8件**（デスクの相棒が反応する場面を想定）
- 道具: `docs/experiments/css-gesture/spike.py` / `run.sh` / `build_viewer.py` / `gestures.css`

## 結果

### Laya (0.42B) は判別しなかった

**8件すべてで `gesture = flicker`**、`emit` は 0.002〜0.112（すべて「反応しない」寄り）。

| state | gesture | top p | emit |
|---|---|---|---|
| fixed_bug | flicker | 0.533 | 0.054 |
| build_fail | flicker | 0.752 | 0.112 |
| waiting | flicker | 0.470 | 0.062 |
| branch_gone | flicker | 0.713 | 0.028 |
| uncertain | flicker | 0.304 | 0.035 |
| late_night | flicker | 0.881 | 0.002 |
| approved | flicker | 0.375 | 0.081 |
| nag | flicker | 0.285 | 0.101 |

確率は動いている（0.285〜0.881）のに、**argmax が定数**。日本語テキストの意味判断には
0.42B では足りない。**「速いから小さいモデル」はこの用途では成立しない。**

### Clef-Flash (9B) は判別した

| state | gesture | top p | conf | energy | approach | emit |
|---|---|---|---|---|---|---|
| fixed_bug（3日のバグが直った） | **nod** | 0.392 | 0.306 | 2.78 | 2.72 | 0.471 |
| build_fail（またビルドが落ちた） | **settle** | 0.548 | 0.483 | 1.06 | 1.09 | 0.356 |
| waiting（テストを待っている） | **idle** | 0.835 | 0.811 | 0.51 | 1.64 | **0.036** |
| branch_gone（ブランチを消された） | **settle** | 0.494 | 0.422 | 0.79 | 0.80 | 0.180 |
| uncertain（次に何をするか不明） | **flicker** | 0.760 | 0.726 | 0.93 | 1.15 | 0.223 |
| late_night（深夜3時） | **idle** | 0.421 | 0.338 | 0.79 | 1.74 | 0.122 |
| approved（レビュー全部OK） | **nod** | 0.831 | 0.807 | 2.22 | 2.91 | 0.435 |
| nag（同じ指摘を3回目） | **settle** | 0.296 | 0.195 | 1.04 | 0.64 | 0.493 |

**8件が5種類のジェスチャーに分かれ、energy / approach も意味の通る向きに出た。**
- 良い知らせ（fixed_bug, approved）で `nod`、energy が高く approach も高い（2.72 / 2.91 = 最も近づく）
- 悪い知らせ（build_fail, branch_gone, nag）で `settle`、energy と approach が低い
  （nag は approach 0.64 で最も遠ざかる）
- 迷い（uncertain）で `flicker`、待ち（waiting）で `idle` + emit 0.036（反応しない）
- `emit` は「反応すべき場面」で高く（0.435〜0.493）、「静かにしているべき場面」で低い（0.036〜0.122）

### θ = 0.5 では何も起きない（較正の実測）

`emit >= 0.5` を発火条件にすると、**8件すべてが idle になった**。Clef の `noul` は
最も明確な場面（nag 0.493）でも 0.5 に届かない。**しきい値は推測せずデータから決める必要がある。**

θ = 0.30 にすると 4件が発火し、`waiting`（raw=idle）と `late_night`（raw=idle）は正しく沈黙する。

### レイテンシ（CPU）

| モデル | 1リクエスト |
|---|---|
| Laya Q8_0 (0.42B) | 1.8〜2.1s |
| Clef-Flash Q4_K_M (9B) | 28.8〜39.6s |

CPU では Clef は実用外。GPU (RTX 3060) なら同条件で **0.43s**（テキストのみ、実測済み）の見込み。
`input_tokens` は 300 前後で、画像を使う場合（631〜1241）より軽い。

## 結論

- **路線としては成立する。** 決定モデルは「どれを・どれくらい」を判別でき、決定論的な写像で
  CSS の動きに落とせる。決定モデルに CSS を生成させる必要は無い（できない）
- **モデル選択が決定的。** Laya (0.42B) は argmax が定数になり使えない。Clef (9B) は判別する。
  「質問が独立に評価される（Laya）」「速い（0.016s）」という利点は、判別できないなら意味が無い
- **`noul` のしきい値はデータから決める。** θ=0.5 は素直だが全部沈黙した
- **設計上のひっかかりを1つ見つけた**: θ でゲートすると `uncertain → flicker` が idle に落ちる。
  **迷いのジェスチャーは本質的に emit が低い**ので、ゲートは「迷いを表現する手段」を真っ先に殺す。
  `flicker` だけゲートを外すか、`emit` の代わりに `confidence` でゲートするかの検討が要る

## 限界

- **人が読み分けられるかは未判定。** 本節の成果は「モデルの出力が意味の通る向きに分かれた」ところまで。
  8個のジェスチャーの混同しやすさ、分布の混合が意味のある中間になるか、confidence が低いとき
  「迷い」に見えるか、score 軸が動きの差として知覚できるか、は `index.html` を見て人が判定する必要がある
- 状態は 8件だけ。日本語のみ（英語での挙動は未確認）
- `score` のレベル刻みは 5 段階だけ。2〜10 の範囲で刻みを変えたときの挙動は未確認
- レイテンシは CPU のみ。GPU での再測は未実施
- Clef は質問を jointly に判断するため、`gesture` と `energy` を同時に聞いたことによる
  相互作用の大きさは未測定（Laya は独立だが判別しなかったので比較できない）
- サンプリング（確率比例でジェスチャーを選ぶ）は試していない。argmax のみ
- decay を使った事後分布の積分（持続状態）は未実装

## 道具

- `docs/experiments/css-gesture/gestures.css` — 8ジェスチャーの語彙（手書き・決定論的）
- `docs/experiments/css-gesture/spike.py` — 決定モデルへの問い合わせ（`run.sh` が起動〜停止を担当）
- `docs/experiments/css-gesture/build_viewer.py` — 結果を目で判定できる HTML にする
- `docs/experiments/css-gesture/results-laya.json` / `results-clef.json` — 生の応答
- `docs/experiments/css-gesture/index.html` — 判定用ビューア

```sh
cd docs/experiments/css-gesture
bash run.sh                       # Laya で計測
MODEL=clef bash run.sh            # Clef で計測
NGPU=99 MODEL=clef bash run.sh    # GPU で計測（vega など）
python3 build_viewer.py --results results-clef.json --compare results-laya.json --theta 0.30
```
