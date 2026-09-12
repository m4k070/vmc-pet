# vmc-pet 設計メモ

## コンセプト

デスクトップに常駐するドットマトリックス表示の電子ペット。
CA/Leniaの場を「体」として直接表示し、体の内部状態=表現、という構成にすることで
別途「見た目を作るレイヤー」を実装しなくても、体の状態がそのまま表現になる。

体(body)の定義:「知能の出力で書き換えられる持続的な状態があり、その状態を通してしか
外界とやり取りできない(観測が制約される)」という条件を満たすもの。物理的な実体である
必要はなく、シミュレーション内の状態でも良い。

## アーキテクチャ

```
[外部環境]                     [体(CA/Lenia場)]
ユーザー入力(クリック/ホバー/キー)      |
        |                            v
        +--------> [IF層] ---> 場に局所的な摂動を注入
                                     |
                                     v
                            [場の状態が更新される]
                                     |
                                     v
                    [ドットグリッド描画] (体の状態=表示)
```

- **常駐ウィンドウ**: デスクトップに透過・枠なし・常に最前面で表示する。
- **ドットグリッド描画**: 32x32のグリッド。各セルの値(0〜1)を円の大きさ・透明度に
  マッピングして描画する。
- **体(CA/Lenia場)**: 既存のLenia実装をそのまま流用。場の値がそのまま描画対象になるため、
  Vに相当するエンコーダを別途実装する必要がない。
- **入力IF**: マウスクリック・ホバーを「場の特定座標にエネルギーを注入する」だけに
  限定する。内部のCAロジックを直接いじらせないための境界線。
  Dockerでの体コンテナ/知能コンテナ分離と同じ発想 — 体の境界を明示的に強制する。
- **表現の方向づけ(任意・後回し可)**: 場の外側に少数の状態変数(例: エネルギー量)を
  持たせ、場のパラメータ(拡散率・成長量など)に影響させると、「元気/放置されて弱る」
  といった情緒的な表現が生まれる。

## 確定事項

### ウィンドウ方式: wlr-layer-shell

開発環境のコンポジタは **niri** (Wayland)。niriはスクロール型タイリングのため、
通常の xdg-toplevel は必ずタイル配置され「透過・枠なし・最前面」を実現できない。
layer-shell surface のみがこの要件を満たす。X11 override-redirect は
xwayland-satellite 経由となり実質的に選択肢から外れる。

| 項目 | 値 |
|---|---|
| layer | `top` (waybarと同格。全画面アプリより下) |
| exclusive_zone | `-1` (他ウィンドウを押しのけない) |
| keyboard_interactivity | `none` |
| namespace | `vmc-pet` (niri側の layer-rule から参照できる) |
| input region | 体のバウンディングボックスのみ (周囲の透明部分はクリックが下に抜ける) |

### 場の解像度と表示解像度の分離

Lenia は kernel radius R が 10 を下回ると離散化誤差で生物が維持できない。
表示解像度(32x32)をそのまま場の解像度にすると R≈5 が上限となり、既存のLenia資産
(`~/work/Lenia/Python/animals.json`)が使えなくなる。

したがって **内部場 96x96 (R=12) → 3x3平均プーリング → 表示 32x32** とする。
プーリングは学習も表現も持たない純粋なダウンサンプルであり、
「Vに相当するエンコーダを実装しない」というコンセプトは維持される。

| パラメータ | 値 |
|---|---|
| 場の解像度 | 96x96 |
| kernel radius R | 12 |
| dt | 0.1 |
| 境界条件 | 周辺減衰 (端から8セルで減衰させ、生物が画面外へ逃げないようにする) |
| 表示解像度 | 32x32 ドット |

### 時間刻み

描画fpsと場の更新レートを分離する。両者を結合すると、fpsを変えた瞬間に
生物の速度と安定性が変わってしまう。

| パラメータ | 値 |
|---|---|
| step_rate | 15 step/s (ポインタ非接触時は落としてCPUを節約) |
| render_fps | 30 fps |

常駐アプリのため、アイドル時 CPU 2%未満・RSS 50MB未満を目標値とする。

### 実装スタック

| レイヤ | 選択 |
|---|---|
| 言語 | Rust (単一バイナリ・低フットプリント) |
| ウィンドウ | `smithay-client-toolkit` (wlr-layer-shell) |
| 描画 | `wl_shm` バッファ + `tiny-skia` (ソフトウェア描画) |
| 場 | 自前実装 (96x96・R=12 の直接畳み込み) |
| 生物データ | `~/work/Lenia/Python/animals.json` を `serde_json` で読む |
| 開発環境 | `flake.nix` devShell |

## モジュール構成

依存方向を一方向に強制する。`body` は Wayland も入力も一切知らない純粋ロジック。

```
src/
  body/          # 体 = 場。外部依存ゼロの純粋ロジック
    field.rs         # Field と純粋な step
    lenia.rs         # カーネル・成長関数・パラメータ
    perturbation.rs  # Perturbation { x, y, radius, amount }
  interface/     # IF層: 外界 → 摂動 だけを通す窓口
    port.rs          # trait BodyPort
    pointer.rs       # ポインタイベント → Perturbation への変換
  render/
    dot_grid.rs      # FieldView → ピクセルバッファ
  shell/
    layer.rs         # OS依存部分(wlr-layer-shell)をここに隔離
  app.rs             # 唯一の可変状態の持ち主。上記を束ねるループ
```

```rust
/// 体の場。値域は 0.0..=1.0 に正規化して保持する
pub struct Field { width: usize, height: usize, cells: Vec<f32> }

/// IF層が体に注入できる唯一の操作
pub struct Perturbation { pub x: usize, pub y: usize, pub radius: f32, pub amount: f32 }

/// 体の境界。将来C(コントローラ)を後付けする際も、人間と同じこの窓口を通す
pub trait BodyPort {
    fn inject(&mut self, perturbation: Perturbation);
    fn observe(&self) -> FieldView<'_>;   // 読み取り専用ビュー。内部 Vec は露出させない
}

/// 入力の意味を型で列挙する(booleanフラグにしない)
pub enum Touch {
    Hover { at: CellPos },   // 弱い持続注入
    Click { at: CellPos },   // 強い単発注入
    Leave,
}
```

`observe()` が `&Field` ではなく `FieldView` を返すのが要点。IF層の向こう側から
場の内部を書き換える経路が型レベルで消え、「内部のCAロジックを直接いじらせない」
という境界が強制される。

## 実装ロードマップ

| # | マイルストーン | 完了条件 |
|---|---|---|
| 0 | flake.nix で devShell | `nix develop` で環境が再現する |
| 1 | layer-shell 常駐ウィンドウ | 透明な矩形が最前面に出て、周囲のクリックが下に抜ける |
| 2 | ドットグリッド描画 | ダミー波形が32x32のドットで30fps描画される |
| 3 | Lenia場を接続 | `animals.json` の生物が場の上で安定移動する |
| 4 | 入力IF | クリックで局所注入され場が反応する。ホバーは弱い持続注入 |
| 5 | (任意) 気分状態 | エネルギー量が growth 係数に効き、放置で弱る |

### 未確定(実装しながら決める)

- 初期生物の選定 — orbium が 96x96 で安定するかは step 3 で実測する
- 摂動量の強さ — 体感チューニングのため設定ファイルに出す
- 気分状態(step 5)の変数設計 — step 3-4 で場の挙動を見てから決める

## 将来の拡張(体を付け替える実験の足場として)

現状のIF層(座標+摂動量だけを受け取る窓口)は、将来Cのようなコントローラを
後付けする際にそのまま入出力インターフェースとして使える設計にしてある。
体を差し替える実験をするなら、この電子ペットが最初の「体」の実例になる。

## 参考にした先行研究

- **World Models** (Ha & Schmidhuber, 2018) — V(エンコーダ)/M(世界モデル)/C(コントローラ)の三分割
- **Growing Neural Cellular Automata** (Mordvintsev et al., Distill 2020) — 体を成長させるCA規則
- **ASAL / ASAL++** (Sakana AI, 2024–2025) — 基盤モデルによるALife発見の自動化
- **NerveNet / MetaMorph** — 体をまたいで共有できるコントローラ設計
- **DERL** (Gupta et al., Nature Communications 2021) — 体の進化と学習の二重ループ
- **Voyager** (MineDojo) — 知能側をAPI(IF)経由でしか環境に触れさせない設計
