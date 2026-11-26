# Quaderno4Linux

富士通 電子ペーパー **QUADERNO** (FMVDP51 など、Sony DPT-RP1 系) を Linux から管理するデスクトップアプリです。\
Rust と [GPUI Kit](https://github.com/longbridge/gpui-kit) (GPUI + GPUI Component) で実装しています。

動作環境、機能などは [Wiki](https://github.com/YutoMaeda1209/quaderno4linux/wiki) を参照してください。

## 開発

### DevContainer

VS Code の Dev Containers 拡張で開くと、GPUI のビルドに必要なライブラリを `mcr.microsoft.com/devcontainers/rust:2.1.7-1-trixie` に加えた環境が起動します。\
原則として、 開発開発は Dev Containers を利用してください。

### ビルド

```sh
cargo build --release
./target/release/quaderno4linux
```

### テスト

```sh
cargo test --workspace         # 単体テスト + モック端末を使った E2E テスト
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

### 構成

| パス | 内容 |
| --- | --- |
| `crates/quaderno-core` | プロトコル実装 |
| `crates/quaderno4linux` | GPUI デスクトップアプリ |
| `crates/quaderno-mock` | 開発・テスト用のモック端末 |
| `packaging/licenses` | ライセンス情報で利用するテンプレート |
| `packaging/linux` | udev ルール、デスクトップエントリ |

### コントリビュート

コントリビュートの方法は [CONTRIBUTING.md](.github/CONTRIBUTING.md) を参照してください。