#!/bin/sh
# Builds the system module: the library's cdylib for wasm32-unknown-unknown,
# copied to target/module/system.wasm, and prints its size and SHA-256 (the
# module hash the hub stores it under, client-compute.md section 2.1).
set -eu

cd "$(dirname "$0")/.."

cargo build -q --locked --release -p system-compiler --lib --target wasm32-unknown-unknown
mkdir -p target/module
cp target/wasm32-unknown-unknown/release/system_compiler.wasm target/module/system.wasm
size=$(wc -c < target/module/system.wasm | tr -d ' ')
hash=$(sha256sum target/module/system.wasm | cut -d ' ' -f 1)
echo "target/module/system.wasm $size bytes sha256 $hash"
