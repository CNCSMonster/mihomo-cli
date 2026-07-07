#!/bin/bash
set -e

TARGETS=(
    "x86_64-unknown-linux-gnu"
    "x86_64-unknown-linux-musl"
    "x86_64-apple-darwin"
    "aarch64-unknown-linux-musl"
    "aarch64-apple-darwin"
    "x86_64-pc-windows-gnu"
)

if ! command -v cargo-zigbuild &>/dev/null; then
    echo "Installing cargo-zigbuild..."
    cargo install cargo-zigbuild
fi

for t in "${TARGETS[@]}"; do
    rustup target add "$t" 2>/dev/null || true
done

echo "=== Building all targets ==="
for t in "${TARGETS[@]}"; do
    echo ""
    echo "Building $t ..."
    cargo zigbuild --target "$t" --release
done

echo ""
echo "=== All binaries ==="
for t in "${TARGETS[@]}"; do
    if [ "$t" = "aarch64-apple-darwin" ]; then
        ls -lh target/release/mihomo-cli 2>/dev/null
    else
        ls -lh "target/$t/release/mihomo-cli"* 2>/dev/null
    fi
done
