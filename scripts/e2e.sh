#!/bin/sh
# End to end test of the system compiler against the real hub, inside one
# container. Uses the hub repository's container harness (scripts/e2e/ in the
# hub, documented in its docs/implementation/local/container-e2e.md) to start
# Redis, SeaweedFS, the hub database and the hub, then runs `serve` against it
# and checks, through the hub's public chunk API:
#
#   - the registry chunk: 202 first, then 200, eleven frames, and the stored
#     section byte-identical to a local compile
#   - every non-empty depth 1 cell of frame 4 and two empty cells: 200 with
#     the immutable cache header, one section per container, the total mass
#     within 0.5 percent of the frame's mass, empty sections where expected
#   - a repeated request served from storage with no new job row
#   - a second submission of the registry answered 409, and a 16 byte section
#     answered 400 with a JSON body carrying "code"
#
# Prints one line per assertion and exits 0 only if every one passed.
#
# Usage: sh scripts/e2e.sh
#
# Environment (all optional):
#   GX_E2E_HUB_SRC     hub repository to copy       (default /context/3GIXHub)
#   GX_E2E_HUB_DIR     writable copy of the hub     (default /tmp/hub)
#   GX_E2E_KEEP_STATE  1 keeps the hub database and object store from a
#                      previous run; by default both are reset so the first
#                      request for each key is a cache miss
#   GX_E2E_TIMEOUT     seconds to wait for each chunk (default 120)
set -u

ROOT_DIR=$(cd "$(dirname "$0")/.." && pwd)
HUB_SRC=${GX_E2E_HUB_SRC:-/context/3GIXHub}
HUB_DIR=${GX_E2E_HUB_DIR:-/tmp/hub}
TIMEOUT=${GX_E2E_TIMEOUT:-120}
ENV_FILE=/tmp/gx-e2e.env
HUB_LOG=/tmp/hub.log
DB=space_runtime
WEED_DIR=/tmp/weed
WORK=$(mktemp -d /tmp/gx-compiler-e2e.XXXXXX)
COMPILER_LOG=$WORK/compiler.log
BIN=$ROOT_DIR/target/release/system-compiler
DATA=$ROOT_DIR/data/system.toml

# Frame and depth under test, and a key no step requests, for the 400 check.
FRAME=4
DEPTH=1
REJECT_KEY=4-5-0-0-0

PGHOST=${PGHOST:-127.0.0.1}
PGPORT=${PGPORT:-5432}
PGUSER=${PGUSER:-postgres}
export PGHOST PGPORT PGUSER

passed=0
failed=0
compiler_pid=

# pass/fail MESSAGE: one summary line per assertion.
pass() {
    passed=$((passed + 1))
    echo "PASS $*"
}
fail() {
    failed=$((failed + 1))
    echo "FAIL $*"
}

# check CONDITION_STATUS MESSAGE: pass when the status is 0.
check() {
    if [ "$1" -eq 0 ]; then
        shift
        pass "$*"
    else
        shift
        fail "$*"
    fi
}

# Stops on a setup failure: no assertion can run after it.
die() {
    echo "error: $*" >&2
    fail "setup: $*"
    finish
}

stop_compiler() {
    if [ -n "$compiler_pid" ] && kill -0 "$compiler_pid" 2>/dev/null; then
        kill "$compiler_pid" 2>/dev/null
        i=0
        while kill -0 "$compiler_pid" 2>/dev/null && [ "$i" -lt 20 ]; do
            i=$((i + 1))
            sleep 1
        done
        kill -9 "$compiler_pid" 2>/dev/null || true
        wait "$compiler_pid" 2>/dev/null
        echo "compiler: stopped (pid $compiler_pid)"
    fi
    compiler_pid=
}

# Stops the compiler and the hub, prints the totals, and exits.
finish() {
    trap - EXIT INT TERM
    stop_compiler
    if [ -f "$COMPILER_LOG" ]; then
        echo "--- compiler log ---"
        cat "$COMPILER_LOG"
        echo "--- end of compiler log ---"
    fi
    if [ -f "$HUB_DIR/scripts/e2e/hub-down.sh" ]; then
        sh "$HUB_DIR/scripts/e2e/hub-down.sh"
    fi
    rm -rf "$WORK"
    echo "e2e: $passed passed, $failed failed"
    if [ "$failed" -eq 0 ] && [ "$passed" -gt 0 ]; then
        exit 0
    fi
    exit 1
}
trap finish EXIT
trap 'echo "interrupted" >&2; failed=$((failed + 1)); finish' INT TERM

for cmd in curl jq psql awk cmp cargo; do
    command -v "$cmd" >/dev/null 2>&1 || die "required command not found: $cmd"
done

# hub_get KEY BODY HEADERS: GETs a chunk through the hub with the API key.
# Prints the HTTP status.
hub_get() {
    curl -s -o "$2" -D "$3" -w '%{http_code}' -H "X-API-Key: $GX_API_KEY" \
        "$GX_HUB_URL/space/$GX_SPACE_ID/build/$GX_BUILD_ID/chunk/$1"
}

# cache_control HEADERS: the Cache-Control value of a saved response.
cache_control() {
    tr -d '\r' <"$1" | sed -n 's/^[Cc]ache-[Cc]ontrol: *//p' | head -n 1
}

# poll KEY BODY HEADERS: GETs once a second until 200 or the timeout. Prints
# the last status.
poll() {
    i=0
    while :; do
        code=$(hub_get "$1" "$2" "$3")
        if [ "$code" = "200" ] || [ "$i" -ge "$TIMEOUT" ]; then
            echo "$code"
            return
        fi
        i=$((i + 1))
        sleep 1
    done
}

# jobs_for KEY: compilation_jobs rows for this build and key.
jobs_for() {
    psql -X -q -t -A -d "$DB" -c \
        "SELECT count(*) FROM compilation_jobs WHERE build_id = '$GX_BUILD_ID' AND chunk_key = '$1'"
}

# 1. The hub: a writable copy, then its harness.
if [ -d "$HUB_DIR" ]; then
    echo "hub: using existing copy at $HUB_DIR"
else
    cp -r "$HUB_SRC" "$HUB_DIR" || die "could not copy $HUB_SRC to $HUB_DIR"
    echo "hub: copied $HUB_SRC to $HUB_DIR"
fi

# A fresh database and object store, so that the first request for every key
# is a miss. Only when nothing is running: services the harness reuses keep
# their state.
hub_up=$(curl -s -o /dev/null -w '%{http_code}' --max-time 2 "http://127.0.0.1:5297/api/health")
s3_up=$(curl -s -o /dev/null -w '%{http_code}' --max-time 2 "http://127.0.0.1:8333/")
if [ "${GX_E2E_KEEP_STATE:-0}" = "1" ]; then
    echo "state: kept (GX_E2E_KEEP_STATE=1)"
elif [ "$hub_up" != "000" ] || [ "$s3_up" != "000" ]; then
    echo "state: hub or object store already running, state kept"
else
    psql -X -q -d postgres -c "DROP DATABASE IF EXISTS $DB" >/dev/null || die "could not drop $DB"
    rm -rf "$WEED_DIR"
    echo "state: reset database $DB and $WEED_DIR"
fi

sh "$HUB_DIR/scripts/e2e/hub-up.sh" || die "hub-up.sh failed"
[ -f "$ENV_FILE" ] || die "$ENV_FILE not written by hub-up.sh"
set -a
. "$ENV_FILE"
set +a

# 2. The compiler.
echo "compiler: building (release)"
(cd "$ROOT_DIR" && cargo build --release --quiet -p system-compiler) || die "compiler build failed"

# Expected values from the same binary and data the daemon uses.
"$BIN" compile --data "$DATA" --key registry --out "$WORK/registry.local" 2>/dev/null ||
    die "local compile of the registry failed"
"$BIN" describe --data "$DATA" --key registry >"$WORK/registry.local.json" ||
    die "local describe of the registry failed"
frame_mass=$(jq -r --argjson f "$FRAME" '.frames[] | select(.frame_id == $f) | .mass_kg' "$WORK/registry.local.json")
[ -n "$frame_mass" ] || die "frame $FRAME not in the registry"

# 3. serve in the background, with the harness's environment.
"$BIN" serve --data "$DATA" >"$COMPILER_LOG" 2>&1 &
compiler_pid=$!
# The hub logs nothing when a compiler connects (its connection registry has
# no logging and ASP.NET Core request logs are at Warning) and has no
# endpoint reporting connected compilers, so the compiler's own log is the
# signal: "connected to hub" follows a completed WebSocket upgrade.
i=0
until grep -q "connected to hub" "$COMPILER_LOG" 2>/dev/null; do
    i=$((i + 1))
    if [ "$i" -gt 60 ] || ! kill -0 "$compiler_pid" 2>/dev/null; then
        cat "$COMPILER_LOG" >&2
        die "compiler did not connect within 60 s"
    fi
    sleep 1
done
pass "compiler connected to $GX_HUB_URL/compiler/connect (pid $compiler_pid)"

# 4. The registry.
code=$(hub_get registry "$WORK/registry.bin" "$WORK/registry.hdr")
[ "$code" = "202" ]
check $? "GET chunk/registry first response: HTTP $code (expected 202)"
code=$(poll registry "$WORK/registry.bin" "$WORK/registry.hdr")
[ "$code" = "200" ]
check $? "GET chunk/registry polled: HTTP $code (expected 200 within $TIMEOUT s)"
if [ "$code" = "200" ]; then
    if "$BIN" describe --key registry --from-file "$WORK/registry.bin" --container >"$WORK/registry.json" 2>"$WORK/err"; then
        frames=$(jq '.frame_count' "$WORK/registry.json")
        sections=$(jq '.section_count' "$WORK/registry.json")
        [ "$frames" = "11" ] && [ "$sections" = "1" ]
        check $? "registry decodes with describe --from-file --container: $sections section, $frames frames (expected 1, 11)"
        jq -e --slurpfile l "$WORK/registry.local.json" '.sections[0] == ($l[0] | del(.key, .bytes, .sha256))' \
            "$WORK/registry.json" >/dev/null
        check $? "registry frames served by the hub equal the local compile"
    else
        fail "registry decodes with describe --from-file --container: $(cat "$WORK/err")"
    fi
    n=$(wc -c <"$WORK/registry.local" | tr -d ' ')
    tail -c "$n" "$WORK/registry.bin" | cmp -s - "$WORK/registry.local"
    check $? "registry section in the container is byte-identical to the local compile ($n bytes)"
fi

# 5. Cells of frame FRAME at depth DEPTH, plus two empty cells: the first two
#    in (x, y, z) order at DEPTH, or at DEPTH + 1 when every cell at DEPTH
#    holds matter.
full=$("$BIN" keys --data "$DATA" --frame "$FRAME" --depth "$DEPTH") || die "keys failed"
[ -n "$full" ] || die "no non-empty keys for frame $FRAME at depth $DEPTH"
empty=
for d in "$DEPTH" $((DEPTH + 1)); do
    listed=$("$BIN" keys --data "$DATA" --frame "$FRAME" --depth "$d")
    side=$((1 << d))
    x=0
    while [ "$x" -lt "$side" ]; do
        y=0
        while [ "$y" -lt "$side" ]; do
            z=0
            while [ "$z" -lt "$side" ]; do
                k="$FRAME-$d-$x-$y-$z"
                if ! echo "$listed" | grep -qx "$k" && [ "$(echo "$empty" | wc -w)" -lt 2 ]; then
                    empty="$empty $k"
                fi
                z=$((z + 1))
            done
            y=$((y + 1))
        done
        x=$((x + 1))
    done
    [ "$(echo "$empty" | wc -w)" -ge 2 ] && break
done
echo "keys: non-empty at depth $DEPTH: $(echo "$full" | tr '\n' ' ')"
echo "keys: empty:$empty"

# Request every key once so the hub dispatches them together, then poll each.
for k in $full $empty; do
    code=$(hub_get "$k" "$WORK/$k.bin" "$WORK/$k.hdr")
    echo "GET chunk/$k first response: HTTP $code, Cache-Control: $(cache_control "$WORK/$k.hdr")"
done

: >"$WORK/masses"
for k in $full $empty; do
    expect_empty=false
    case " $empty " in *" $k "*) expect_empty=true ;; esac
    code=$(poll "$k" "$WORK/$k.bin" "$WORK/$k.hdr")
    cache=$(cache_control "$WORK/$k.hdr")
    [ "$code" = "200" ]
    check $? "GET chunk/$k: HTTP $code (expected 200 within $TIMEOUT s)"
    [ "$code" = "200" ] || continue
    [ "$cache" = "public, max-age=31536000, immutable" ]
    check $? "chunk/$k Cache-Control: $cache"
    if "$BIN" describe --key "$k" --from-file "$WORK/$k.bin" --container >"$WORK/$k.json" 2>"$WORK/err"; then
        sections=$(jq '.section_count' "$WORK/$k.json")
        is_empty=$(jq '.sections[0].empty' "$WORK/$k.json")
        mass=$(jq '.mass_kg' "$WORK/$k.json")
        [ "$sections" = "1" ] && [ "$is_empty" = "$expect_empty" ]
        check $? "chunk/$k decodes: $sections section, empty $is_empty (expected 1, $expect_empty), mass $mass kg"
        [ "$expect_empty" = "true" ] || echo "$mass" >>"$WORK/masses"
    else
        fail "chunk/$k decodes with describe --from-file --container: $(cat "$WORK/err")"
    fi
done

# Total mass over the non-empty cells against the frame's registry mass.
count=$(wc -l <"$WORK/masses" | tr -d ' ')
result=$(awk -v m="$frame_mass" '{ s += $1 } END { e = (s - m) / m; if (e < 0) e = -e;
    printf "%.9e %.6f %d\n", s, 100 * e, (e <= 0.005) ? 0 : 1 }' "$WORK/masses")
total=$(echo "$result" | cut -d ' ' -f 1)
pct=$(echo "$result" | cut -d ' ' -f 2)
ok=$(echo "$result" | cut -d ' ' -f 3)
[ "$count" = "$(echo "$full" | wc -l | tr -d ' ')" ] && [ "$ok" = "0" ]
check $? "mass over $count non-empty depth $DEPTH cells of frame $FRAME: $total kg vs registry $frame_mass kg, error $pct percent (limit 0.5)"

# 6. A repeated request is served from storage: 200 at once, no new job row.
k=$(echo "$full" | head -n 1)
before=$(jobs_for "$k")
code=$(curl -s -o "$WORK/again.bin" -D "$WORK/again.hdr" -w '%{http_code} %{time_total}' \
    -H "X-API-Key: $GX_API_KEY" "$GX_HUB_URL/space/$GX_SPACE_ID/build/$GX_BUILD_ID/chunk/$k")
after=$(jobs_for "$k")
seconds=${code#* }
code=${code%% *}
cache=$(cache_control "$WORK/again.hdr")
[ "$code" = "200" ] && [ "$cache" = "public, max-age=31536000, immutable" ]
check $? "GET chunk/$k again: HTTP $code in $seconds s, Cache-Control: $cache"
[ -n "$before" ] && [ "$before" = "$after" ]
check $? "compilation_jobs rows for chunk/$k: $before before, $after after (expected unchanged)"
cmp -s "$WORK/again.bin" "$WORK/$k.bin"
check $? "chunk/$k body identical to the first 200"

# 7. Direct submissions with the compiler's credentials.
submit() {
    curl -s -o "$WORK/submit.out" -w '%{http_code}' -X POST \
        -H "X-API-Key: $GX_API_KEY" -H "X-Compiler-Secret: $GX_COMPILER_SECRET" \
        -H 'X-Layer-Section-Type: matter' -H 'Content-Type: application/octet-stream' \
        --data-binary "@$2" \
        "$GX_HUB_URL/space/$GX_SPACE_ID/build/$GX_BUILD_ID/chunk/$1/compiled"
}
code=$(submit registry "$WORK/registry.local")
[ "$code" = "409" ]
check $? "POST chunk/registry/compiled with the same registry bytes again: HTTP $code (expected 409)"

head -c 16 /dev/zero >"$WORK/zeros.bin"
code=$(submit "$REJECT_KEY" "$WORK/zeros.bin")
body=$(cat "$WORK/submit.out")
[ "$code" = "400" ] && jq -e 'has("code")' "$WORK/submit.out" >/dev/null 2>&1
check $? "POST chunk/$REJECT_KEY/compiled with 16 zero bytes: HTTP $code, body $body (expected 400 with \"code\")"

# 8. The compiler must still be running: nothing above should have stopped it.
kill -0 "$compiler_pid" 2>/dev/null
check $? "compiler still running after every request"

finish
