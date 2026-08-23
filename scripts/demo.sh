#!/usr/bin/env bash
# Render the README tour.
#
# The published GIF used to come from a tape carrying an absolute path into a
# scratch directory from whichever session produced it, with the mp4->gif step
# living only in someone's shell history. It could not be regenerated. This
# script is the whole recipe: containers, demo home, render, convert, optimise.
#
#   ./scripts/demo.sh              render docs/media/essh-demo.gif
#   ./scripts/demo.sh --setup      (re)build and start the three demo hosts
#   KEEP=1 ./scripts/demo.sh       leave the scratch home behind for debugging
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$PWD"
SCRATCH="${TMPDIR:-/tmp}/essh-demo.$$"
DEMO_HOME="$SCRATCH/home"
KEY="${SSH_KEY:-$HOME/.ssh/id_ed25519}"
HOSTS=(web-01:2201:bookworm web-02:2202:bookworm web-03:2203:bullseye)

cleanup() { [ -n "${KEEP:-}" ] || rm -rf "$SCRATCH"; }
trap cleanup EXIT

need() { command -v "$1" >/dev/null || { echo "need $1" >&2; exit 1; }; }
need vhs; need ffmpeg; need docker

# ── the three hosts ────────────────────────────────────────────────────────
# Two bookworm, one bullseye. The odd one out is what the fleet screen finds:
# an older OS, older OpenSSL, older packages, and an nginx.conf edited by hand.
setup_hosts() {
  need gifsicle
  cp "$KEY.pub" docs/demo/authorized_keys
  docker build -q --build-arg BASE=debian:12 -t essh-demo:bookworm docs/demo
  docker build -q --build-arg BASE=debian:11 -t essh-demo:bullseye docs/demo
  rm -f docs/demo/authorized_keys
  for spec in "${HOSTS[@]}"; do
    IFS=: read -r name port tag <<<"$spec"
    docker rm -f "$name" >/dev/null 2>&1 || true
    docker run -d --name "$name" -p "$port:22" "essh-demo:$tag" >/dev/null
  done
  sleep 3
  # The hand edit that gives web-03 a config hash of its own.
  ssh -p 2203 -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
    root@127.0.0.1 \
    'echo "# hand-edited during an incident" >> /etc/nginx/nginx.conf' 2>/dev/null
  echo "hosts up"
}

if [ "${1:-}" = "--setup" ]; then setup_hosts; exit 0; fi

for spec in "${HOSTS[@]}"; do
  IFS=: read -r name port _ <<<"$spec"
  ssh -p "$port" -o BatchMode=yes -o StrictHostKeyChecking=no \
      -o UserKnownHostsFile=/dev/null -o ConnectTimeout=4 \
      root@127.0.0.1 true 2>/dev/null \
    || { echo "$name is not reachable on $port — run: $0 --setup" >&2; exit 1; }
done

# ── a throwaway home, so recording never touches a real ~/.essh ────────────
mkdir -p "$DEMO_HOME/.essh"
cp -R "$HOME/.ssh" "$DEMO_HOME/.ssh"
cat > "$DEMO_HOME/.essh/config.toml" <<EOF
theme = "dark"
host_groups = []

[general]
default_user = "root"
default_key = "$KEY"
tofu_policy = "auto"
launcher = true
connect_timeout = 12

[session]
prefix_key = "ctrl-a"
max_concurrent = 9

[[hosts]]
name = "web-01"
hostname = "127.0.0.1"
port = 2201
user = "root"
tags = { role = "web", site = "syd" }

[[hosts]]
name = "web-02"
hostname = "127.0.0.1"
port = 2202
user = "root"
tags = { role = "web", site = "syd" }

[[hosts]]
name = "web-03"
hostname = "127.0.0.1"
port = 2203
user = "root"
tags = { role = "web", site = "mel" }
EOF

cargo build --release
HOME="$DEMO_HOME" ./target/release/essh workspace save production \
  web-01 web-02 web-03 >/dev/null

# ── render ─────────────────────────────────────────────────────────────────
# The tape carries @HOME@ rather than a path, so it stays reproducible.
sed -e "s#@HOME@#$DEMO_HOME#g" -e "s#@OUT@#$SCRATCH/tour.gif#g" \
  docs/media/tour.tape > "$SCRATCH/tour.tape"
( cd "$ROOT" && vhs "$SCRATCH/tour.tape" )

gifsicle -O3 --lossy=45 --colors 160 "$SCRATCH/tour.gif" -o docs/media/essh-demo.gif
ffmpeg -y -loglevel error -i "$SCRATCH/tour.gif" \
  -movflags faststart -pix_fmt yuv420p \
  -vf "scale=trunc(iw/2)*2:trunc(ih/2)*2" docs/media/essh-demo.mp4

printf 'gif  %s  %s\n' "$(du -h docs/media/essh-demo.gif | cut -f1)" \
  "$(ffprobe -v error -show_entries format=duration -of csv=p=0 docs/media/essh-demo.gif)s"
printf 'mp4  %s\n' "$(du -h docs/media/essh-demo.mp4 | cut -f1)"
