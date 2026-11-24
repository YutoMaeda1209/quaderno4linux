FROM mcr.microsoft.com/devcontainers/rust:2.1.7-1-trixie

# GPUI のビルドに必要なネイティブライブラリ。
# libssl-dev、libfontconfig-dev、libfreetype-dev、libxcb1-dev、libzstd-dev は
# ベースイメージに含まれている。
RUN apt-get update \
  && export DEBIAN_FRONTEND=noninteractive \
  && apt-get install -y --no-install-recommends \
    clang cmake \
    libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libx11-xcb-dev \
    libvulkan-dev \
  && rm -rf /var/lib/apt/lists/*

# コンテナ内で GUI を動かすための Vulkan ドライバと日本語フォント。
RUN apt-get update \
  && export DEBIAN_FRONTEND=noninteractive \
  && apt-get install -y --no-install-recommends \
    mesa-vulkan-drivers fonts-noto-cjk \
  && rm -rf /var/lib/apt/lists/*

# リリースを .deb と .rpm にパッケージするツール。
RUN cargo install --locked cargo-deb@3.8.0 cargo-generate-rpm@0.21.1 \
  && rm -rf /usr/local/cargo/registry
