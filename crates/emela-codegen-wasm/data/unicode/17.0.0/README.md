# Unicode 17.0.0 のデータ

WASM の runtime が使う書記素クラスタの表（UAX #29）の出所．**Unicode 17.0.0**（2025 年）の
公式データをそのまま置き，`python3 scripts/gen-unicode-tables.py` が
`src/emit/runtime/unicode_tables.rs` を生成する．生成はネットワークを使わない（下のファイルだけを
読む）．CI は `python3 scripts/gen-unicode-tables.py --check` で生成をやり直し，commit 済みの表と
一致することを確かめる（テストは commit 済みの表を使う）．

| ファイル | 出所 |
| --- | --- |
| `GraphemeBreakProperty.txt` | <https://www.unicode.org/Public/17.0.0/ucd/auxiliary/GraphemeBreakProperty.txt>（`GraphemeBreakProperty-17.0.0.txt`，Date: 2025-06-30） |
| `GraphemeBreakTest.txt` | <https://www.unicode.org/Public/17.0.0/ucd/auxiliary/GraphemeBreakTest.txt>（`GraphemeBreakTest-17.0.0.txt`，Date: 2025-03-24） |
| `emoji-data.txt` | <https://www.unicode.org/Public/17.0.0/ucd/emoji/emoji-data.txt>（`emoji-data.txt`，Date: 2025-07-25．`Extended_Pictographic` だけを使う） |
| `InCB.txt` | `DerivedCoreProperties-17.0.0.txt`（<https://www.unicode.org/Public/17.0.0/ucd/DerivedCoreProperties.txt>，Date: 2025-07-30）の `Indic_Conjunct_Break` の行だけを抜き出したもの．抽出方法はファイルの先頭に書いてある（データ行は変更していない）． |

`InCB` は `GraphemeBreakProperty.txt` には無く `DerivedCoreProperties.txt` にある．1 MB を超える
後者の全体を vendor せず，UAX #29 GB9c に要る `InCB` の行だけをそのまま置いている．

## 生成

```sh
python3 scripts/gen-unicode-tables.py
```

このスクリプトは上のファイルだけを読み，`crates/emela-codegen-wasm/src/emit/runtime/unicode_tables.rs`
を書き換える．出力は決定的（同じ入力なら同じバイト列）．データを更新するときは，新しい版の
ディレクトリを `data/unicode/<version>/` に足し，`scripts/gen-unicode-tables.py` の
`VERSION` を変える．

`GraphemeBreakTest.txt` の全件は `src/emit/runtime/unicode.rs` の unit test（Rust の参照実装）と
`tests/unicode.rs`（生成した Emela プログラムを wasm で実行）が読む．

## ライセンス

Unicode のデータファイルは Unicode License V3（<https://www.unicode.org/license.txt>）．
`LICENSE.txt` に全文を置く．生成した `src/emit/runtime/unicode_tables.rs` の先頭にも，この
ライセンスへの参照を置く．© 2025 Unicode, Inc.「Unicode and the Unicode Logo are registered
trademarks of Unicode, Inc. in the U.S. and other countries.」
