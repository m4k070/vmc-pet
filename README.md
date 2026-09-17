# vmc-pet

デスクトップに常駐する、ドットマトリックス表示の電子ペット。

連続セルオートマトン [Lenia](https://github.com/Chakazul/Lenia) の場をそのまま
「体」として表示する。体の内部状態がそのまま見た目になるので、別途「見た目を作る
レイヤー」を実装する必要がない。

![動作中の様子。デスクトップ右下に半透明のドットマトリックスとして常駐し、
Orbium が場の上を漂っている](docs/media/demo.webp)

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

生物は `--animal <code>` で選べる(デフォルトは Orbium unicaudatus)。

```sh
cargo run --release -- --list-animals   # 選べる生物の一覧
cargo run --release -- --animal OG2g    # Gyrorbium gyrans で起動
```

【実験】多チャンネル Lenia の生物(Chan 氏のリポジトリの探索結果から、ペットの試験一式に合格した
9体)を、ペットの仕組みにはつながずに表示することもできる([docs/experiments/rule-candidates.md](docs/experiments/rule-candidates.md))。

```sh
cargo run --release -- --list-multichannel            # 表示できる生物の一覧
cargo run --release -- --preview-multichannel 231-04  # 64x64 の場で表示する(クリックで突ける)
cargo run --release -- --multichannel 231-04          # 元気・テンポ・クリックだけつないで動かす
cargo run --release -- --multichannel 231-04 --preview-mood disappointed  # がっかりのテンポで動かす
```

【実験】閉じた場を動き回る粒子の体([docs/experiments/body-candidates.md](docs/experiments/body-candidates.md))を、
探索の候補番号で表示することもできる。ポインタを乗せるとその点へ誘い、クリックすると弾く。

```sh
cargo run --release -- --preview-particles 1091                     # 32x32 の箱で表示する
cargo run --release -- --preview-particles 1091 --particle-zoom 2   # 16x16 の箱を2倍に拡大して表示する
cargo run --release -- --preview-particles 1091 --particle-log ~/particles.csv  # 1秒ごとの状態と操作を記録する
```

記録は、ユーザーの操作を含めた状況を後から調べるためのもので、その場では学習しない。集計は
`cargo run --release -p vmc-pet-body --example particle_trial -- log ~/particles.csv` で行う。

`--multichannel` では、放っておくと約150秒でエネルギーが尽きて体が弱り、クリックすると回復する。
慣れ・色素・生活リズムの学習・自律コントローラ・記憶はまだつないでいない。

体の上にポインタを乗せると撫でている扱いになり、触れた場所がほんのり光る
(見た目だけの反応で、体そのものには影響しない)。クリックすると突いた扱いになり、
体に実際にエネルギーが注入されて反応する。Lenia はカオス系なので触り方によっては
体が崩壊するが、そのときは自動で生物を置き直す。

マシンの CPU 負荷も体の外側にある環境として反映される。負荷が高いほどエネルギーの
減衰が速くなり(環境が厳しいという扱い)、放置されて弱りやすくなる。この負荷は
`/proc/stat` の全コア合算値なので、マルチコア機では1コアだけを張り付かせても
影響はわずかになる。コア数によって体感が変わる点は [docs/DESIGN.md](docs/DESIGN.md)
を参照。

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
| 5 | 気分状態による表現の方向づけ | 完了 |

常駐時の実測値 (release, 2560x1440@60Hz、検証機は Intel Core i5-10400
6コア12スレッド): CPU 1.9% / RSS 4.5MB / 30.00fps。

体には触れずに触れられないまま放置すると、エネルギーが徐々に減っていき、
自己修復の力が弱まって総量がわずかに下がる(崩れはしない)。クリックすれば
エネルギーが回復し、元の元気さに戻っていく。

## 構成

依存方向を一方向に強制してある。`vmc-pet-body` は Wayland も入力も一切知らない
純粋ロジックで、独立したクレートに切り出してある(体を差し替える実験の足場。
[docs/M5STACK.md](docs/M5STACK.md) 参照)。

```
crates/
  vmc-pet-body/    # 体 = 場。外部依存ゼロの純粋ロジック。std/no_std 両対応
    src/
      field.rs         # Field / FieldView
      lenia.rs         # カーネル・成長関数・更新規則
      animal.rs        # animals.json の読み込みと RLE デコード
      perturbation.rs  # Perturbation / CellPos / accumulate_into(共有ヘルパー)
      port.rs          # trait BodyPort (体が外に見せる唯一の窓口)
      lenia_body.rs    # BodyPort の実装。崩壊からの復帰も持つ
      math.rs          # 数学関数のシム(std/no_stdで実装を切り替える)
src/               # デスクトップ版バイナリ(vmc-pet-body に依存)
  interface/       # IF層: 外界 → 体・echoが受け取れる形 だけを通す
    pointer.rs       # Touch → (体への摂動, echoへの摂動)
    machine_load.rs  # CPU負荷 → 環境ストレス(スカラー)
  render/
    dot_grid.rs      # 体+echo → ピクセルバッファ
    camera.rs        # 重心を画面中央に置く表示原点(場は書き換えない)
    touch_echo.rs    # 入力の可視化専用データ。体には一切影響しない
  shell/
    layer.rs         # OS依存部分(wlr-layer-shell)をここに隔離
  cli.rs             # コマンドライン引数の解釈
  app.rs             # 唯一の可変状態の持ち主
```

体の境界は `FieldView` が担保している。場の読み取り専用ビューであり内部の `Vec` を
露出しないため、外側から場を直接書き換える経路が型のレベルで存在しない。

設計上の判断と、その根拠になった実測値は [docs/DESIGN.md](docs/DESIGN.md) にある。
ペット本体の設計を変えなかった実験(崩壊の先読み、体の規則の候補、表現軸を広げる試み)の記録は
[docs/experiments/](docs/experiments/README.md) にある。
周辺減衰が生物を殺すこと、場を 32x32 まで縮めても挙動が変わらないこと、表示原点の
丸めが毎フレームの揺れを生むことなどは、いずれも計測して決めている。

## 出典

`assets/animals.json` の生物データは Bert Chan 氏の
[Lenia](https://github.com/Chakazul/Lenia) (MIT License) から4体を抜き出したもの。
詳細は [assets/NOTICE.md](assets/NOTICE.md) を参照。

実験用の Glaberish の更新式(`Lenia::step_glaberish`)と、半径ごとの形からカーネルを作る方法
(`Lenia::with_radial_profile`)、`examples/glaberish_trial.rs` の s613 の規則は、Q. Tyrell Davis 氏と
Josh Bongard 氏の研究([Glaberish](https://arxiv.org/abs/2205.10463))と、その実装
[yuca](https://github.com/riveSunder/yuca) (MIT License, Copyright (c) 2022 riveSunder) による。
