FROM mcr.microsoft.com/devcontainers/base:trixie

# Native libraries required to build GPUI (X11 / Wayland / Vulkan / fonts) and
# the TLS stack, plus tools for running the GUI headless (Xvfb) during development.
RUN apt-get update \
  && export DEBIAN_FRONTEND=noninteractive \
  && apt-get install -y --no-install-recommends \
    build-essential pkg-config clang cmake \
    libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev \
    libx11-xcb-dev libxcb1-dev libvulkan-dev mesa-vulkan-drivers vulkan-tools \
    libfontconfig-dev libfreetype-dev fonts-noto-cjk \
    libssl-dev libzstd-dev \
    xvfb xdotool imagemagick x11-utils \
    iproute2 avahi-utils usbutils \
  && apt-get clean \
  && rm -rf /var/lib/apt/lists/*
