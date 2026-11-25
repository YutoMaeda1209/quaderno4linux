# Quaderno4Linux

富士通 電子ペーパー **QUADERNO** (FMVDP51 など、Sony DPT-RP1 系) を Linux から管理するデスクトップアプリです。\
Rust と [GPUI Kit](https://github.com/longbridge/gpui-kit) (GPUI + GPUI Component) で実装しています。

機能、動作環境、インストール方法、使い方、トラブルシューティング、プロトコルの概要は [Wiki](https://github.com/YutoMaeda1209/quaderno4linux/wiki) を参照してください。

## ビルド

Rust (stable) をインストールしたうえで、ビルドに必要なライブラリを入れます。

```sh
# Ubuntu / Debian
sudo apt install build-essential pkg-config clang cmake \
  libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libx11-xcb-dev \
  libxcb1-dev libvulkan-dev mesa-vulkan-drivers libfontconfig-dev \
  libfreetype-dev libssl-dev libzstd-dev

# Fedora
sudo dnf install gcc gcc-c++ clang cmake pkgconf-pkg-config \
  libxkbcommon-devel libxkbcommon-x11-devel wayland-devel libxcb-devel \
  vulkan-loader-devel mesa-vulkan-drivers fontconfig-devel freetype-devel \
  openssl-devel libzstd-devel
```

```sh
cargo build --release
./target/release/quaderno4linux
```

USB 接続に必要な udev ルールとデスクトップエントリの設定は [Wiki のインストール](https://github.com/YutoMaeda1209/quaderno4linux/wiki/インストール) を参照してください。

## 開発

### DevContainer

VS Code の Dev Containers 拡張で開くと、GPUI のビルドに必要なライブラリを `mcr.microsoft.com/devcontainers/rust:2.1.7-1-trixie` に加えた環境が起動します。\
コンテナ内ではビルドとテスト ([モック端末](https://github.com/YutoMaeda1209/quaderno4linux/wiki/モック端末) を使った E2E テストを含む) を行い、GUI の動作確認や実機との接続はホストで行ってください。

### テスト

```sh
cargo test --workspace         # 単体テスト + モック端末を使った E2E テスト
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

### 構成

| パス | 内容 |
| --- | --- |
| `crates/quaderno-core` | プロトコル実装 (USB モード切替、Bluetooth PAN、mDNS 検索、ペアリング、認証、文書 API) |
| `crates/quaderno4linux` | GPUI デスクトップアプリ |
| `crates/quaderno-mock` | 開発・テスト用のモック端末 |
| `packaging/linux` | udev ルール、デスクトップエントリ |

コントリビュートの方法は [CONTRIBUTING.md](.github/CONTRIBUTING.md) を参照してください。
