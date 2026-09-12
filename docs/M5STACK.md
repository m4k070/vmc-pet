# M5Stack を第二の体として実装する計画

> `docs/DESIGN.md` の「将来の拡張(体を付け替える実験の足場として)」で見据えていた
> 「体を差し替える実験」の第一弾。PC上のウィンドウ(既存のvmc-pet)とは別に、
> M5Stack 単体で完結する**もう一つの体**を作る。

## 位置づけ: PC版の表示器ではなく、独立した第二の体

M5Stack-chan の既存の顔アバターを PC 側 Lenia の気分状態(`energy` など)で
ネットワーク越しに操る案(以下「案B」)も検討したが、採らない。それは
「場の状態→表情への翻訳レイヤー」を新たに作ることになり、`docs/DESIGN.md`
コンセプト節が最初から避けたかった「別途見た目を作るレイヤー」を持ち込んでしまう。

採用するのは、**M5Stack が Lenia の場を自前で計算し、自前の画面に描く、独立した体**
という案(以下「案A」)。PC版と同じ Lenia の規則を使うが、両者は互いに通信しない
別々のインスタンスになる。「体の状態がそのまま表現になる」というコンセプトの核を
壊さずに済む。

## 対象ハードウェア: M5Stack CoreS3 (確定)

| 項目 | 値 |
|---|---|
| モデル | M5Stack CoreS3 |
| チップ | ESP32-S3(Xtensa) |
| 画面 | 320x240 (ILI9342C系)。現行の 32x32 グリッドを 1セル=7px 相当で描ける
  (PC版の 384x384 ウィンドウよりむしろ余裕がある) |
| 入力 | 静電容量タッチ + 電源ボタン(物理 A/B/C ボタンは無い) |

このサンドボックス環境に実機を USB 接続して確認した。ESP32-S3 内蔵の
USB-JTAG/シリアルデバッグユニット(VID:PID `303a:1001`、Espressif)として
認識され、`/dev/ttyACM1` にシリアルポートが生える(`dialout` グループに
入っていれば追加設定なしでアクセスできる)。

## ツールチェーンの現実(このサンドボックスで検証済み)

`nix eval` で確認した限り、以下は nixpkgs にある。

| パッケージ | 役割 |
|---|---|
| `espflash` (4.4.0) | ビルド済みバイナリを実機に書き込む・シリアル監視する |
| `espup` (0.17.1) | Xtensa 対応 Rust ツールチェーンをインストールする公式ツール |
| `rustup` (1.29.0) | espup が前提とする Rust ツールチェーン管理。素の nixpkgs の
  rustc/cargo は espup が認識しないため別途要る |
| `esp-generate` (1.3.0) | esp-hal ベースのプロジェクト雛形を生成する |
| `ldproxy` (0.31.4) | esp-idf(std)ビルド時のリンカプロキシ |

一方で **`esp-idf` 本体は nixpkgs に無い**。ESP32-S3 は Xtensa アーキテクチャ
であり、upstream の rustc は Xtensa をターゲットにできないため、`espup` が取得する
Espressif フォークの rustc が必須になる。この取得は Nix のビルドサンドボックス
の外側で行う一回限りのインストール作業(`espup install`)で、PC版の `flake.nix`
のような「`nix develop` するだけで完全に再現される」体験にはならない。

**実際にこのサンドボックスで `espup install --targets esp32s3` を実行し、
ESP32-S3 向け Xtensa Rust ツールチェーン(`xtensa-esp32s3-none-elf` を含む)の
インストールを完了した。** `dl.espressif.com` / `github.com` への到達性があれば
動く。所要時間・ダウンロード量はネットワーク次第(このサンドボックスでは
数分程度)。

書き込み(`espflash flash`)・実機での動作確認は、実機がこのマシンに接続されて
いる間はここでも試せるが、恒常的な開発は素直にユーザー自身のマシンで行うのが
現実的(このサンドボックスは会話が終われば消える)。

## std (esp-idf-hal) か no_std (esp-hal) か

| | esp-idf-hal (std) | esp-hal (no_std, 推奨) |
|---|---|---|
| `body/` の再利用 | ほぼそのまま(f32 の `.sqrt()` 等が使える) | 数学関数だけ差し替えが要る(下記) |
| ビルドの重さ | ESP-IDF(C SDK)を追加取得。ネットワーク依存 | 純粋 Rust のみ。取得は Xtensa rustc だけ |
| 開発の勢い | 安定しているが esp-rs の主軸は no_std 側に移りつつある | 活発に開発が進んでいる |
| WiFi等が要るなら | 素直 | 後から `esp-wifi` crate を足せる |

今回は「M5Stack自身のCPU負荷」のような外部入力は使わない(後述)ため WiFi は
不要で、esp-idf という重い C SDK を抱える理由が無い。**no_std (`esp-hal`) を推奨**する。

### no_std にすると何が壊れるか(実測済み)

`#![no_std]` の下で `f32` のどのメソッドが使えるかを、実際にコンパイルして確認した。

```rust
// core だけで使える(変更不要)
x.abs()  x.clamp(a, b)  x.max(y)  x.min(y)
i32::rem_euclid  // 整数版はOK

// std が要る(no_std ではコンパイルエラー)
x.sqrt()  x.cos()  x.exp()  x.powi(n)  x.floor()  x.ceil()  x.round()
```

`body/` 内でこれらが実際に使われている箇所を数えると、影響は小さい
(テストコードのみで使う `.hypot()` `.round()` は本番ビルドには含まれない)。

| メソッド | 使用箇所 |
|---|---|
| `.sqrt()` | field.rs, lenia.rs(×2), perturbation.rs |
| `.cos()` | perturbation.rs(`weight_at`) |
| `.exp()` | lenia.rs(`KernelCore::Exponential` / `GrowthMapping::Exponential`) |
| `.powi()` | lenia.rs(`KernelCore::Polynomial` / `GrowthMapping::Polynomial`) |
| `.floor()` | lenia.rs(カーネルのリング判定) |
| `.ceil()` | perturbation.rs(`accumulate_into` の到達半径) |

合計6箇所・5種類のメソッド。`libm` crate(no_std 対応の libm 実装、これも
esp-hal のエコシステムでよく使われる)の自由関数(`libm::sqrtf` 等)に置き換える
だけで済む。`powi` だけは libm に相当品が無いので、`x*x*x*x` のような手書きの
掛け算に展開する(元々指数は 4 固定なので難しくない)。

対応方針は、呼び出し側を直接 `libm::sqrtf(x)` に書き換えるのではなく、
`body` 内に小さな数学シム(`fn sqrtf(x: f32) -> f32` 等)を1つ置き、
`std`/`no_std` を cargo feature で切り替える形にする。呼び出し側のコードは
`x.sqrt()` から `sqrtf(x)` への書き換えで済み、PC版・M5Stack版で同じ
`body` クレートのソースを共有できる(予測可能性: 同じ処理は同じパターンで書く)。

## 埋め込みに伴う簡略化

- `assets/animals.json` の JSON + serde 読み込みと RLE デコーダ(`animal.rs`)は
  フラッシュ・RAM の節約のため M5Stack 版では使わない。4体切り替えは不要で、
  Orbium 1体分のパラメータとパターンを Rust の const 配列として埋め込む
  (CLI 引数での生物切り替えは PC版だけの機能に留める)
- 場のサイズは PC版と同じ 32x32 のまま(R=13 なら約440タップ、1ステップあたり
  約45万回の積和。ESP32 のクロックなら 15 step/s でも十分に余裕がある見込みだが、
  実機での実測は未検証)
- `render/dot_grid.rs` のピクセルフォーマット(premultiplied ARGB8888、`wl_shm` 用)
  は Wayland 固有なので流用しない。`embedded-graphics` crate 経由で RGB565 へ
  描く新しいレンダラを書く。プーリング(`pool_cell`)や色のブレンドの考え方
  (体の色とechoの色を混ぜる)はロジックとして流用できる
- `shell/layer.rs`(wlr-layer-shell)は無関係。M5Stack側の「shell」に相当するのは
  SPI 画面への書き込みとボタン/タッチのポーリングを回すメインループだけで、
  コンポジタに相当するものは存在しない

## 入力(IF層)の作り直しが必要な点

マウスのホバー/クリックという概念は物理ボタン・タッチパネルにそのまま
写像できない。

- **Core(物理ボタンA/B/C)**: ボタン押下を「決まった位置へのクリック」に
  割り当てるか、ボタンで走査位置を送り(十字キー的に)決定ボタンでクリックする
- **Core2/CoreS3(静電容量タッチ)**: 画面座標をそのまま `DotGrid::cell_at`
  相当の逆写像でセル座標に変換できる。PC版の `interface::pointer` に最も近い

いずれのモデルかで実装がかなり変わるため、**モデルが確定してから
`interface` 相当のモジュールを設計する**。

## 「環境ストレス」に相当する入力の再検討

PC版の `interface::machine_load`(CPU負荷→エネルギー減衰)は、M5Stack という
別の体にはそのまま持ち込めない。M5Stack 自身の CPU 負荷を読むことは技術的には
できるが、意味が無い(常時ほぼアイドルの組み込み用途であり、「機械が忙しい」が
起こりにくい)。この体で「環境ストレス」に何を割り当てるかは、この体を作る際に
改めて考える(未確定。候補: バッテリー残量、周囲温度・照度センサ〈搭載モデルのみ〉、
何も割り当てず気分状態はタッチ/ボタンの頻度だけで決める、など)。

## 段階的ロードマップ

| # | マイルストーン | 状態 |
|---|---|---|
| 0 | モデル確定・`espup install` でツールチェーン用意 | 完了(CoreS3、`espup install --targets esp32s3`) |
| 1 | `body` を独立クレートへ切り出す(PC版の挙動は変えない) | 完了(`crates/vmc-pet-body`) |
| 2 | 数学シム(`sqrtf` 等)を feature flag で追加し、`no_std` ビルドが通ることを確認 | 完了(`cargo check --no-default-features --target xtensa-esp32s3-none-elf -Z build-std=core,alloc` が警告無しで通る) |
| 3 | Orbium 1体を const 配列で埋め込み、シリアル出力で総量を確認しながら1ステップ動かす | 未着手 |
| 4 | `embedded-graphics` で画面へドット描画 | 未着手 |
| 5 | 物理ボタン/タッチを `Touch` へ変換する IF 層 | 未着手 |
| 6 | (任意)「環境ストレス」の意味づけを考え直して実装する | 未着手 |

step 2 の完了にあたり、`no_std` 下では `Vec`/`String`/`format!`/`vec!` が
標準のプレリュードに無く、`extern crate alloc;` した上で `alloc::vec::Vec` 等を
明示的に import する必要があることも分かった(std 環境でも同じ import は
無害に共存できるため、`std`/`no_std` で分岐させる必要は無かった)。

## 未確定・要相談事項

- 表示解像度に応じた見た目の再チューニング(320x240 は PC版よりゆとりがあるため、
  ドットの隙間比率や縁のぼかし幅を調整し直す余地がある)
- 常時給電か、バッテリー動作を考慮するか
- 「環境ストレス」に何を割り当てるか(前述)
- 実機への書き込み(`espflash flash`)・実際の画面表示・タッチ入力の確認は
  まだ行っていない(コンパイルの確認までが済んだ段階)
