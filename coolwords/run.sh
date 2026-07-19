#!/usr/bin/env bash
# Add-on entrypoint: read options, point both the Rust server and the Python
# importer at /share, sanity-check the dictionary, then exec the server as PID 1.
set -euo pipefail

OPTS=/data/options.json
# Must stay 0.0.0.0 so Home Assistant ingress (the sidebar panel) can reach the
# server over the container network; 127.0.0.1 would break ingress. Publishing the
# port to the LAN is controlled separately by `ports:` in config.yaml, not by this.
BIND="0.0.0.0:7575"
if [ -f "$OPTS" ]; then
  BIND="$(jq -r '.bind // "0.0.0.0:7575"' "$OPTS")"
fi

export COOLWORDS_BIND="$BIND"
export COOLWORDS_DB=/share/coolwords/coolwords.db
export COOLWORDS_USER_DB=/share/coolwords/user.db
export COOLWORDS_BOOKS_DIR=/share/coolwords/books
export COOLWORDS_PYTHON=python3

mkdir -p /share/coolwords/books

if [ ! -f "$COOLWORDS_DB" ]; then
  echo "[coolwords] FATAL: $COOLWORDS_DB not found."
  echo "[coolwords] Seed /share/coolwords/ with coolwords.db (+ coolwords_emb.npy for"
  echo "[coolwords] clustering / level-3 stemming), then restart the add-on."
  echo "[coolwords] Copy via the Samba or SSH add-on into \\\\<ha>\\share\\coolwords\\."
  sleep 10
  exit 1
fi
if [ ! -f /share/coolwords/coolwords_emb.npy ]; then
  echo "[coolwords] WARN: coolwords_emb.npy missing — importing books will fail at the"
  echo "[coolwords] clustering step. Copy it next to coolwords.db to enable full import."
fi

cd /app
echo "[coolwords] starting on ${COOLWORDS_BIND} (db=${COOLWORDS_DB})"
exec ./coolwords_ui
