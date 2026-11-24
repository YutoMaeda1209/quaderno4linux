# AGENTS.md

富士通 QUADERNO (Sony DPT-RP1 系の電子ペーパー) を Linux から管理するデスクトップアプリです。\
Rust と GPUI Kit で実装しています。機能や使い方は [README.md](README.md) を参照してください。

## 構成

- `crates/quaderno-core`: プロトコル実装 (USB モード切替、Bluetooth PAN、\
  mDNS 検索、ペアリング、認証、文書 API)。GUI に依存しない
- `crates/quaderno4linux`: GPUI Kit のデスクトップアプリ
  - `app.rs`: 接続とレイアウト
  - `explorer.rs`: ファイル一覧と操作
  - `dialogs.rs`: ダイアログと通知
  - `tasks.rs`: バックグラウンド処理
- `crates/quaderno-mock`: 開発とテスト用のモック端末。E2E テスト (`tests/end_to_end.rs`) もここにある
- `packaging/linux`: udev ルールとデスクトップエントリ
- `packaging/licenses`: 依存クレートのライセンス一覧 (`THIRD_PARTY_LICENSES.html`) を生成する cargo-about の設定とテンプレート。\
  リリースのワークフローで生成し、deb / rpm に同梱する

プロトコルの変更は `quaderno-core` に入れ、必要ならモック端末も合わせて更新してください。

## ビルドとテスト

ビルドツールは DevContainer に入っています。ホストからは次の形で実行します。

```sh
devcontainer exec --workspace-folder . <コマンド>
```

変更後は CI と同じ次のコマンドをすべて通してください。\
CI は `CARGO_BUILD_WARNINGS=deny` で、Cargo のマニフェストの警告を含めて warning を 1 件も許容しません。

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## 動作確認

- GUI と実機との接続はホストで確認します。\
  コンテナでビルドした `./target/debug/quaderno4linux` をホストでそのまま起動できます。
- 実機がない場合はモック端末を使います。

  ```sh
  cargo run -p quaderno-mock -- 127.0.0.1 12345678   # アドレス, PIN
  cargo run -p quaderno4linux -- --connect 127.0.0.1
  ```

- 認証情報は `~/.config/quaderno4linux/` に保存されます。\
  モック端末とペアリングすると実機の認証情報が上書きされるので、モックで試すときは `XDG_CONFIG_HOME` を一時ディレクトリに向けてください。
- ログは `~/.local/state/quaderno4linux/quaderno4linux.log` に出ます (起動ごとに上書き)。

## 実装の決まり

- UI は GPUI Kit の部品で組み、同等の部品があるものを自作しないでください。\
  import は `gpui_kit::` と `gpui_kit::component::` を使います\
  (`gpui` / `gpui_component` は直接使わない)。
- 通信など時間のかかる処理は `run_task` (`tasks.rs`) でバックグラウンドスレッドに出し、UI スレッドをブロックしないでください。\
  同時に実行できる処理は 1 つです。
- ユーザーへの通知は `notify_error` / `notify_warning` / `notify_success` を使います。\
  エラー通知には要約だけを出し、詳細 (`detail`) はログファイルに送ります。
- 確認や入力のダイアログは `confirm` / `prompt_text` (`dialogs.rs`) を使います。
- UI の文言は日本語です。
- コメントは [CONTRIBUTING.md](.github/CONTRIBUTING.md) の「コードについて」に従って書いてください。
- `packaging/linux/70-quaderno.rules` はアプリに埋め込まれて (`include_str!`) インストールされます。\
  変更するとアプリの挙動も変わります。
- `rust-embed` の `debug-embed` は外さないでください。\
  コンテナでビルドした debug バイナリをホストで動かしたときに、アイコンを表示するのに必要です。

## コミットと PR

ブランチの作成、コミット、PR の作成の前に、必ず\
[CONTRIBUTING.md](.github/CONTRIBUTING.md) を読んで従ってください。
