FROM mcr.microsoft.com/devcontainers/rust:2.1.7-1-trixie

# Native libraries required to build GPUI.
# libssl-dev, libfontconfig-dev, libfreetype-dev, libxcb1-dev and libzstd-dev
# are already provided by the base image.
RUN apt-get update \
  && export DEBIAN_FRONTEND=noninteractive \
  && apt-get install -y --no-install-recommends \
    clang cmake \
    libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libx11-xcb-dev \
    libvulkan-dev \
  && rm -rf /var/lib/apt/lists/*
