# Quaderno4Linux

富士通 電子ペーパー **QUADERNO** (FMVDP51 など、Sony DPT-RP1 系) を Linux から管理するデスクトップアプリです。
Rust と [GPUI](https://www.gpui.rs/) (+ [gpui-component](https://github.com/longbridge/gpui-component)) で実装しています。

## 機能

- **接続**
  - USB: 端末を USB シリアルモードから USB Ethernet モード (RNDIS / CDC-ECM) に切り替え、IPv6 リンクローカルで自動接続
  - Wi-Fi / LAN: mDNS (`_dp_fujitsu._tcp`) で端末を自動検索。IP アドレスを直接指定することも可能
  - ペアリング: 端末に表示される 8 桁の PIN で登録 (認証情報は `~/.config/quaderno4linux/` に保存。[dpt-rp1-py](https://github.com/janten/dpt-rp1-py) の `~/.config/dpt/` があればそれも利用)
- **ファイルエクスプローラー**
  - アップロード (ファイル・フォルダ、ウィンドウへのドラッグ＆ドロップにも対応) / ダウンロード (フォルダは再帰的に)
  - フォルダの作成
  - 削除・コピー・移動 (切り取り → 貼り付け)・名前の変更
  - バッテリー残量・空き容量・ファームウェアバージョンの表示

| 操作 | ショートカット |
| --- | --- |
| 更新 | `F5` / `Ctrl+R` |
| 親フォルダへ | `Backspace` / `Alt+↑` |
| 開く (フォルダ) / ダウンロード (文書) | `Enter` / ダブルクリック |
| すべて選択 / 複数選択 | `Ctrl+A` / `Ctrl+クリック`, `Shift+クリック` |
| コピー / 切り取り / 貼り付け | `Ctrl+C` / `Ctrl+X` / `Ctrl+V` |
| 名前の変更 | `F2` |
| 削除 | `Delete` |
| 新しいフォルダ | `Ctrl+Shift+N` |
| アップロード / ダウンロード | `Ctrl+U` / `Ctrl+D` |

## 動作環境

Ubuntu 24.04 以降 / Fedora 40 以降 (X11 または Wayland)。Vulkan 対応の GPU ドライバ (Mesa など) が必要です。
ファイル選択ダイアログには xdg-desktop-portal (GNOME / KDE / GTK のいずれかの実装) を使います。

## ビルド

Rust (stable) をインストールしたうえで、ビルドに必要なライブラリを入れます。

```sh
# Ubuntu / Debian
sudo apt install build-essential pkg-config clang cmake \
  libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libx11-xcb-dev libxcb1-dev \
  libvulkan-dev mesa-vulkan-drivers libfontconfig-dev libfreetype-dev libssl-dev libzstd-dev

# Fedora
sudo dnf install gcc gcc-c++ clang cmake pkgconf-pkg-config \
  libxkbcommon-devel libxkbcommon-x11-devel wayland-devel libxcb-devel \
  vulkan-loader-devel mesa-vulkan-drivers fontconfig-devel freetype-devel openssl-devel libzstd-devel
```

```sh
cargo build --release
./target/release/quaderno4linux
```

### USB 接続のための設定 (初回のみ)

USB モードの切り替えには端末のシリアルポート (`/dev/ttyACM*`) への書き込み権限が必要です。
同梱の udev ルールをインストールするか、ユーザーを `dialout` グループに追加してください。

```sh
sudo cp packaging/linux/70-quaderno.rules /etc/udev/rules.d/
sudo udevadm control --reload && sudo udevadm trigger
```

デスクトップエントリ: `packaging/linux/io.github.yutomaeda1209.quaderno4linux.desktop` を `~/.local/share/applications/` にコピーします。

## 使い方

1. **USB**: 端末をケーブルでつなぎ「USB で接続」を押します。うまくいかない場合は方式ボタンで RNDIS / CDC-ECM を切り替えてください。
2. **Wi-Fi**: 端末とパソコンを同じネットワークにつなぎ「端末を検索」を押して、見つかった端末をダブルクリックします。見つからない場合は端末の IP アドレスを入力して「接続」を押します。
3. 初回はペアリングを求められます。端末の画面に表示された PIN を入力してください。
4. 右側のエクスプローラーでファイルを操作します。

### トラブルシューティング

- **USB で端末が応答しない**: USB Ethernet のインターフェース (`enx…` / `usb0` など) が有効になっている必要があります。
  NetworkManager が DHCP 失敗でインターフェースを切断してしまう場合は、リンクローカル専用の接続を作成してください。

  ```sh
  nmcli connection add type ethernet con-name Quaderno ifname <インターフェース名> \
    ipv4.method link-local ipv6.method link-local
  ```
- **アドレスの手動指定**: `192.168.0.20`、`fe80::xxxx%enx0123456789ab` (IPv6 リンクローカルはインターフェース名付き) の形式で入力できます。
- **ログ**: `RUST_LOG=debug quaderno4linux` で詳細なログを出力します。

## 開発

### DevContainer

VS Code の Dev Containers 拡張で開くと、`mcr.microsoft.com/devcontainers/rust:2.1.7-1-trixie` に
GPUI のビルドに必要なライブラリを加えた環境が起動します。コンテナ内ではビルドとテスト
(後述のモック端末を使った E2E テストを含む) を行い、GUI の動作確認や実機との接続はホストで行ってください。

### モック端末

実機がなくても開発できるよう、端末をシミュレートするサーバー (`quaderno-mock`) を用意しています。
ペアリング (PIN)・認証・文書 API を実装し、ファイルはメモリ上に保持します。

```sh
cargo run -p quaderno-mock -- 127.0.0.1 12345678   # アドレス, PIN
cargo run -p quaderno4linux -- --connect 127.0.0.1  # 別ターミナルで
```

### テスト

```sh
cargo test --workspace         # 単体テスト + モック端末を使った E2E テスト
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

### libc のバージョン固定について

gpui 0.2.2 が依存する `xattr 0.2.3` は libc 0.2.190 以降でビルドできないため、
`crates/quaderno4linux/Cargo.toml` で `libc = "=0.2.188"` に固定しています。gpui が新しい xattr に更新されたら固定を外してください。

### 構成

| パス | 内容 |
| --- | --- |
| `crates/quaderno-core` | プロトコル実装 (USB モード切替、mDNS 検索、ペアリング、認証、文書 API) |
| `crates/quaderno4linux` | GPUI デスクトップアプリ |
| `crates/quaderno-mock` | 開発・テスト用のモック端末 |
| `packaging/linux` | udev ルール、デスクトップエントリ |

### プロトコルについて

[QUADERNO のプロトコル調査メモ](https://gist.github.com/YutoMaeda1209/d51764c40666c02df871c711d32ed837) と
[dpt-rp1-py](https://github.com/janten/dpt-rp1-py) を参考にしています。概要:

- USB: シリアル (`04c5:1656`) に `01 00 00 01 00 00 00 01 <00=RNDIS|01=CDC-ECM> 04` を書き込むと USB Ethernet (`04c5:1657`) に切り替わる
- ポート 8080 (HTTP): 端末情報・ペアリング (DH group 14 + PBKDF2-HMAC-SHA256 + AES-128-CBC 鍵ラップ)
- ポート 8443 (HTTPS, 自己署名証明書): RSA 署名による認証後、`Credentials` クッキーで文書 API を利用

コントリビュートの方法は [CONTRIBUTING.md](.github/CONTRIBUTING.md) を参照してください。
