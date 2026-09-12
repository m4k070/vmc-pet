# assets/animals.json の出典

`animals.json` に含まれる生物のパラメータとセル配置は、Bert Chan 氏による
Lenia リポジトリの `Python/animals.json` から 4 体分を抜き出したもの。

- 出典: https://github.com/Chakazul/Lenia
- ライセンス: MIT License, Copyright (c) 2018 Bert Chan

セル配置の文字列は Lenia 独自の RLE 形式のまま保持しており、
デコードは `src/body/animal.rs` が行う。
