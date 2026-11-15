#!/bin/sh
# Local and CI check: format, lint, the module build and its determinism
# harness, tests, dash check. Exits non-zero on the first failure.
set -eu

cd "$(dirname "$0")/.."

echo "== cargo fmt"
cargo fmt --all -- --check

echo "== cargo clippy"
cargo clippy --workspace --all-targets -- -D warnings

echo "== module build"
sh scripts/build-module.sh

echo "== module determinism harness"
# The core library's harness (core docs/modules.md): every vector in
# conformance/ through the native module host and through Node. It runs from
# the core checkout cargo already fetched for the gx-core dependency, or from
# GX_CORE_DIR when set.
core=${GX_CORE_DIR:-}
if [ -z "$core" ]; then
    manifest=$(cargo metadata --format-version 1 --locked |
        tr ',' '\n' |
        sed -n 's/^"manifest_path":"\(.*\)\/crates\/gx-core\/Cargo.toml"$/\1/p' |
        head -n 1)
    core=$manifest
fi
if [ ! -f "$core/scripts/module-node.mjs" ]; then
    echo "core library checkout not found (set GX_CORE_DIR)" >&2
    exit 1
fi
cargo build -q --release --manifest-path "$core/Cargo.toml" \
    -p gx-module-host --bin gx-module-run --target-dir target/harness
status=0
target/harness/release/gx-module-run target/module/system.wasm conformance || status=1
node "$core/scripts/module-node.mjs" target/module/system.wasm conformance || status=1
if [ "$status" -ne 0 ]; then
    echo "module determinism: FAILED" >&2
    exit 1
fi

echo "== cargo test"
cargo test --workspace

echo "== dash check"
# No em or en dashes in anything this repository holds.
if grep -rnI "$(printf '\342\200\224')\|$(printf '\342\200\223')" \
    --exclude-dir=target --exclude-dir=.git --exclude=Cargo.lock . ; then
    echo "em or en dash found" >&2
    exit 1
fi

echo "ci: ok"
