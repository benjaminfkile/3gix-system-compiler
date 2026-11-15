#!/bin/sh
# Builds the system module and publishes it to a hub (client-compute.md
# section 2.1): uploads the module bytes (PUT /module), then puts layer
# `system` of a space with the module's hash (PUT /space/{id}/layer/system).
#
# Reads everything that names a hub from the environment; nothing of it is
# in this repository:
#
#   GX_HUB_URL    hub base URL, without a trailing slash
#   GX_API_KEY    an API key with the admin capability (X-API-Key)
#   GX_SPACE_ID   the space to put the layer in
#
# The layer config: attachment "all", usesOrigin true (the registry is
# integrated to the space's origin), spanSeconds 0 (the matter never
# changes), no source manifest, and the max depth table of every frame from
# the registry (`system-compiler max-depth`).
set -eu

cd "$(dirname "$0")/.."

: "${GX_HUB_URL:?set GX_HUB_URL}"
: "${GX_API_KEY:?set GX_API_KEY}"
: "${GX_SPACE_ID:?set GX_SPACE_ID}"

sh scripts/build-module.sh
wasm=target/module/system.wasm
hash=$(sha256sum "$wasm" | cut -d ' ' -f 1)

echo "== PUT /module"
reply=$(curl -sS --fail-with-body -X PUT \
    -H "X-API-Key: $GX_API_KEY" \
    -H "Content-Type: application/wasm" \
    --data-binary "@$wasm" \
    "$GX_HUB_URL/module")
echo "$reply"
case "$reply" in
    *"$hash"*) ;;
    *) echo "the hub returned a hash other than $hash" >&2; exit 1 ;;
esac

max_depth=$(cargo run -q --locked --release -p system-compiler -- max-depth)
config=$(printf '{"module":"%s","sourceManifest":null,"attachment":"all","exclusive":false,"maxDepth":%s,"spanSeconds":0,"usesOrigin":true}' \
    "$hash" "$max_depth")

echo "== PUT /space/{id}/layer/system"
echo "$config"
curl -sS --fail-with-body -X PUT \
    -H "X-API-Key: $GX_API_KEY" \
    -H "Content-Type: application/json" \
    --data-binary "$config" \
    "$GX_HUB_URL/space/$GX_SPACE_ID/layer/system"
echo
