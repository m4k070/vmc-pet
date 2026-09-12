# vmc-pet

デスクトップに常駐する、ドットマトリックス表示の電子ペット。

連続セルオートマトン [Lenia](https://github.com/Chakazul/Lenia) の場をそのまま
「体」として表示する。体の内部状態がそのまま見た目になるので、別途「見た目を作る
レイヤー」を実装する必要がない。

## コンセプト

ここでの体(body)の定義は「知能の出力で書き換えられる持続的な状態があり、その状態を
通してしか外界とやり取りできない(観測が制約される)」というもの。物理的な実体である
必要はなく、シミュレーション内の状態でもよい。

```
[外部環境]                     [体(Lenia場)]
ユーザー入力(クリック/ホバー)          |
        |                            v
        +--------> [IF層] ---> 場に局所的な摂動を注入
                                     |
                                     v
                            [場の状態が更新される]
                                     |
                                     v
                    [ドットグリッド描画] (体の状態=表示)
```

入力は「場の特定座標にエネルギーを注入する」ことだけに限定してあり、内部の CA
ロジックを直接いじる経路はない。この境界は型で強制している(後述)。

将来コントローラ(知能側)を後付けするときも、人間の入力と同じこの窓口を通す。
体を差し替える実験をするなら、この電子ペットが最初の「体」の実例になる。

## 動かす

`rustc` / `cargo` は Nix の devShell で用意する。

```sh
nix develop            # 開発環境に入る
cargo run              # 起動
cargo test             # テスト
cargo clippy           # lint
```

devShell に入らず直接実行してもよい。

```sh
nix develop --command cargo run --release
```

起動すると画面右下に半透明のドットグリッドが常駐する。周囲の透明部分はクリックが
下のウィンドウへ抜ける。終了は `Ctrl-C`。

体の上にポインタを乗せると撫でている扱いになり、触れた場所がほんのり光る
(見た目だけの反応で、体そのものには影響しない)。クリックすると突いた扱いになり、
体に実際にエネルギーが注入されて反応する。Lenia はカオス系なので触り方によっては
体が崩壊するが、そのときは自動で生物を置き直す。

## 動作環境

`wlr-layer-shell` に対応した Wayland コンポジタが必要。niri 26.04 で動作を確認して
いる。sway / Hyprland など wlroots 系でも動くはずだが未確認。

X11 と、layer-shell を実装しない GNOME (Mutter) では動作しない。

| 項目 | 値 |
|---|---|
| layer | `top` |
| namespace | `vmc-pet` |
| exclusive zone | `-1` (他のウィンドウを押しのけない) |
| 入力領域 | 体のバウンディングボックスのみ |

niri 側で挙動を調整する場合は namespace で拾える。

```kdl
layer-rule {
  match namespace="vmc-pet"
  block-out-from "screencast"
}
```

## 現在の状態

| # | マイルストーン | 状態 |
|---|---|---|
| 0 | Nix devShell | 完了 |
| 1 | layer-shell 常駐ウィンドウ | 完了 |
| 2 | ドットグリッド描画 | 完了 |
| 3 | Lenia 場を体として接続 | 完了 |
| 4 | 入力を局所摂動として注入する IF 層 | 完了 |
| 5 | (任意) 気分状態による表現の方向づけ | 未着手 |

常駐時の実測値 (release, 2560x1440@60Hz): CPU 1.6% / RSS 4.4MB / 30.00fps。

## 構成

依存方向を一方向に強制してある。`body` は Wayland も入力も一切知らない純粋ロジック。

```
src/
  body/            # 体 = 場。外部依存ゼロ
    field.rs         # Field / FieldView
    lenia.rs         # カーネル・成長関数・更新規則
    animal.rs        # animals.json の読み込みと RLE デコード
    perturbation.rs  # Perturbation / CellPos / accumulate_into(共有ヘルパー)
    port.rs          # trait BodyPort (体が外に見せる唯一の窓口)
    lenia_body.rs    # BodyPort の実装。崩壊からの復帰も持つ
  interface/       # IF層: 外界 → 摂動 だけを通す
    pointer.rs       # Touch → (体への摂動, echoへの摂動)
  render/
    dot_grid.rs      # 体+echo → ピクセルバッファ
    camera.rs        # 重心を画面中央に置く表示原点(場は書き換えない)
    touch_echo.rs    # 入力の可視化専用データ。体には一切影響しない
  shell/
    layer.rs         # OS依存部分(wlr-layer-shell)をここに隔離
  app.rs             # 唯一の可変状態の持ち主
```

体の境界は `FieldView` が担保している。場の読み取り専用ビューであり内部の `Vec` を
露出しないため、外側から場を直接書き換える経路が型のレベルで存在しない。

設計上の判断と、その根拠になった実測値は [docs/DESIGN.md](docs/DESIGN.md) にある。
周辺減衰が生物を殺すこと、場を 32x32 まで縮めても挙動が変わらないこと、表示原点の
丸めが毎フレームの揺れを生むことなどは、いずれも計測して決めている。

## 出典

`assets/animals.json` の生物データは Bert Chan 氏の
[Lenia](https://github.com/Chakazul/Lenia) (MIT License) から4体を抜き出したもの。
詳細は [assets/NOTICE.md](assets/NOTICE.md) を参照。
