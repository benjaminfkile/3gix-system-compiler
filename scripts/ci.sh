#!/bin/sh
# Local and CI check: format, lint, tests. Exits non-zero on the first failure.
set -eu

cd "$(dirname "$0")/.."

echo "== cargo fmt"
cargo fmt --all -- --check

echo "== cargo clippy"
cargo clippy --workspace --all-targets -- -D warnings

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
