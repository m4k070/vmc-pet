# assets/animals.json の出典

`animals.json` に含まれる生物のパラメータとセル配置は、Bert Chan 氏による
Lenia リポジトリの `Python/animals.json` から 4 体分を抜き出したもの。

- 出典: https://github.com/Chakazul/Lenia
- ライセンス: MIT License, Copyright (c) 2018 Bert Chan

セル配置の文字列は Lenia 独自の RLE 形式のまま保持しており、
デコードは `src/body/animal.rs` が行う。

# assets/multichannel.json の出典

`multichannel.json` に含まれる多チャンネル Lenia の生物のパラメータとセル配置は、同じ
Lenia リポジトリの `Python/found/221.json`・`222.json`・`231.json` から 9 体分を抜き出したもの
(各生物の `source` に元のファイルと番号を書いてある)。パラメータとセル配置は元のまま変えておらず、
呼び名の `id` と `source` だけを足した。

- 出典: https://github.com/Chakazul/Lenia
- ライセンス: MIT License, Copyright (c) 2018 Bert Chan

デコードと更新式は `crates/vmc-pet-body/src/multichannel.rs` が行う。
