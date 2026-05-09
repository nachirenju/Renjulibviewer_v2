# Renjulibviewer v2

[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg)](https://www.gnu.org/licenses/gpl-3.0)
[![Rust](https://img.shields.io/badge/rust-2024-orange.svg)](https://www.rust-lang.org)

**Renjulibviewer v2** は、連珠（五目並べ）の定石ファイル（.lib）や Yixin データベース（.db）を高速に閲覧・編集するために設計された、モダンなクロスプラットフォーム・アプリケーションです。

---

## ✨ 主な機能

- 💻 **マルチプラットフォーム**: Windows デスクトップ版および Web ブラウザ版 (WebAssembly) に対応。
- 📂 **幅広いファイル形式**: RenLib形式（.lib）および Yixin DB形式（.db）の読み込み・保存をフルサポート。
- 🧩 **高度な分岐表示**: 定石の分岐点をドット（通常点・対称点・合流点）やラベルで直感的に可視化。
- 🌍 **多言語・多エンコーディング**: 日/英/中/韓のUI切替、および各種テキストエンコーディング（Shift_JIS, UTF-8等）に対応し、文字化けを解消。
- 🔗 **外部連携・出力**:
  - 局面の **SGF出力**
  - **Renjuportal (V1/V2)** 形式のリンク自動生成
  - スクリーンショット・GIFキャプチャ機能

---

## 🛠 ビルド方法

ビルドには最新の [Rust](https://www.rust-lang.org/tools/install) ツールチェーンが必要です。

### 1. デスクトップ版
Windows 等のバイナリを生成します。

```bash
git clone https://github.com/your-username/Renjulibviewer_v2.git
cd Renjulibviewer_v2
cargo build --release
```
実行ファイルは `target/release/` 内に生成されます。

### 2. Web版 (Wasm)
[Trunk](https://trunkrs.dev/) を使用してビルド・実行します。

```bash
# trunk のインストール
cargo install --locked trunk

# ローカルサーバーの起動
trunk serve
```
起動後、ブラウザで `http://localhost:8080` へアクセスしてください。

---

## ⌨️ 操作方法

### 基本操作
| アクション | 操作 |
| :--- | :--- |
| **石を置く / 分岐に進む** | 左クリック |
| **一手戻る** | 右クリック |
| **最初に戻る / 最後まで進む** | ツールバーの `|<` / `>|` |
| **テキスト・ラベル編集** | `Ctrl` + 左クリック |
| **分岐の削除** | `Delete` キー（確認ダイアログあり） |

### ツールバー
- 📁: ファイルを開く
- 💾: 名前を付けて保存
- ⚙: 設定ウィンドウ（盤のデザイン、エンコーディング、言語設定など）

---

## ⚙️ 設定項目

設定画面では、ユーザーの好みに合わせた柔軟なカスタマイズが可能です。
- **デザイン**: 盤の色、石の質感（リアルな陰影 or モダンなフラットデザイン）を選択。
- **テキスト**: 盤上に配置するラベルの色やフォント設定。
- **互換性**: ファイル内のコメントが文字化けする場合、言語設定から適切なエンコーディングを選択してください。

---

## ⚖️ ライセンス

### ソフトウェア
**GNU General Public License version 3 (GPL v3)**  
詳細は [LICENSE](LICENSE) ファイルをご参照ください。

### フォント
- **[Noto Sans CJK (Japanese / Simplified Chinese / Traditional Chinese / Korean)](https://fonts.google.com/noto)**  
  Licensed under the [SIL Open Font License, Version 1.1](http://scripts.sil.org/OFL).
  Subsetted files provided by [taka4sato/NotoSans-SubsetFont](https://github.com/taka4sato/NotoSans-SubsetFont).

Developed by **nachirenju**
