#!/bin/bash

set -euo pipefail

CRATE_NAME=$(grep -m 1 '^name =' Cargo.toml | cut -d '"' -f 2)
echo "📦 Detected binary name: $CRATE_NAME"

mkdir -p dist

declare -A builds
builds["x64"]="x86_64-unknown-linux-gnu"
builds["arm64"]="aarch64-unknown-linux-musl"

export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld

for arch in "${!builds[@]}"; do
  target="${builds[$arch]}"

  echo "🔨 Building for $target (linux-$arch)..."

  rustup target add --toolchain nightly "$target" >/dev/null 2>&1 || true

  RUSTFLAGS="-Zlocation-detail=none -Zunstable-options -Cpanic=immediate-abort" \
    cargo +nightly build --release \
      -Z build-std=std,panic_abort \
      -Z build-std-features=optimize_for_size \
      --target "$target"

  src="target/$target/release/$CRATE_NAME"
  dst="dist/${CRATE_NAME}-linux-${arch}"

  if [ -f "$src" ]; then
    cp "$src" "$dst"
    echo "✅ Copied: $dst"
  else
    echo "❌ Binary not found at $src"
    exit 1
  fi
done

echo "🎉 Done! Binaries are in ./dist/"
ls -lh dist/