# FileFinder_H

指定したフォルダ以下のファイルをあいまい検索して、タイムラインに挿入する AviUtl2 プラグイン（`.aux2`）。
Rusty Scripts Search（aviutl2-rs の examples/scripts-search-plugin）の検索対象を「エフェクト」から「ファイル」に置き換え、
Everything（voidtools）の機能の一部（フィルタ・並べ替え・検索構文・リアルタイム更新・ドラッグ・実行回数）を足したもの。

- aviutl2-rs **0.47**（本体 **2.1.10 以上**）
- デプロイ先: `Plugin/FileFinder_H/FileFinder_H.aux2`
- 設定: `Plugin/FileFinder_H/config.json`（フォルダ・拡張子・フィルタ・並べ替え）
- 挿入履歴: `Plugin/FileFinder_H/history.json`
- ユーザー向けの使い方: `自作スクリプトマニュアル/FileFinder_H_Manual.md`

## 構成

| ファイル | 役割 |
|---|---|
| `src/index.rs` | 別スレッドでの走査。サイズ・更新日時・並べ替えキーも持つ。50 万件で打ち切り |
| `src/query.rs` | Everything の `ext:` `size:` `dm:` を解釈し、残りをあいまい検索（nucleo）へ |
| `src/watcher.rs` | notify（ReadDirectoryChangesW）で監視。変更が 1.5 秒止むか最初の変更から 20 秒で全体を読み込み直す |
| `src/dnd.rs` | シェルのデータオブジェクト + `SHDoDragDrop` で一覧からドラッグ。戻ったら WM_LBUTTONUP を送り直す |
| `src/history.rs` | 挿入・ドロップの回数と最終時刻 |
| `src/gui.rs` | 画面。本体への書き込み（`call_edit_section`）はクリックと Enter のときだけ |
| `src/kana.rs` | カナ正規化（scripts-search-plugin から写したもの） |

## ビルド

```powershell
& "C:\ProgramData\aviutl2\AI\plugins\FileFinder_H\build.ps1"
cargo test   # このフォルダで
```

## 出典

`src/kana.rs` の正規化と画面の作りは aviutl2-rs の examples/scripts-search-plugin から写した
（Copyright (c) 2025 Nanashi. / MIT License）。検索構文は Everything（https://www.voidtools.com/）の書き方に合わせた。
