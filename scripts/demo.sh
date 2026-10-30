#!/bin/bash
# Two-process encrypted sync on one Mac. Builds the debug binary if needed.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
if ! command -v cargo >/dev/null 2>&1; then
  # shellcheck disable=SC1090
  . "$HOME/.cargo/env"
fi

cargo build --manifest-path "$root/Cargo.toml" -p photosync
bin="$root/target/debug/photosync"

work="$(mktemp -d "${TMPDIR:-/tmp}/photosync-demo.XXXXXX")"
trap 'rm -rf "$work"' EXIT

hub="$work/hub"
client="$work/client"
mkdir -p "$hub" "$client/album"
printf 'mov-bytes' > "$hub/clip.mov"
printf 'jpeg-bytes' > "$client/album/pic.jpg"
printf 'not media' > "$client/notes.txt"

psk="$("$bin" keygen)"
cat > "$work/hub.toml" <<EOF
role = "hub"
folder = "$hub"
listen = "127.0.0.1:0"
psk_hex = "$psk"
EOF
cat > "$work/client.toml" <<EOF
role = "client"
folder = "$client"
peer = "127.0.0.1:9"
psk_hex = "$psk"
EOF

"$bin" hub --config "$work/hub.toml" > "$work/hub.out" 2> "$work/hub.err" &
hub_pid=$!
for _ in 1 2 3 4 5 6 7 8 9 10; do
  if grep -q PHOTOSYNC_LISTEN "$work/hub.out" 2>/dev/null; then
    break
  fi
  sleep 0.2
done
addr="$(awk '/PHOTOSYNC_LISTEN / { print $2 }' "$work/hub.out")"
if [[ -z "${addr}" ]]; then
  echo "hub did not print a listen address" >&2
  cat "$work/hub.err" >&2
  kill "$hub_pid" 2>/dev/null || true
  exit 1
fi

cat > "$work/client.toml" <<EOF
role = "client"
folder = "$client"
peer = "$addr"
psk_hex = "$psk"
EOF

echo "first sync"
"$bin" client --config "$work/client.toml"
echo "second sync"
"$bin" client --config "$work/client.toml"
kill "$hub_pid" 2>/dev/null || true
wait "$hub_pid" 2>/dev/null || true

test -f "$hub/album/pic.jpg"
test -f "$client/clip.mov"
test ! -f "$hub/notes.txt"
echo "demo ok: $work"
