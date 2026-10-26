#!/usr/bin/env bash
# Laya prototype run (decision rule: tools/fieldref-prereg.md; run only after the owner has
# confirmed the review). CPU, 2 threads on cores 6,7, no network, models read-only.
# Usage: tools/run_laya.sh [out-dir]   (default /media/data/mimik/laya/runs/<stamp>)
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
MODELS=/media/data/mimik/laya/laya-7b928d82
OUT=${1:-/media/data/mimik/laya/runs/$(date +%Y%m%d-%H%M)}
(cd "$MODELS" && sha256sum -c --quiet SHA256SUMS)
mkdir -p "$OUT"
docker run --rm --init --pull never --name mimik-laya-run --network none --user 1000:1000 \
  --cap-drop ALL --security-opt no-new-privileges --read-only --tmpfs /tmp -e HF_HOME=/tmp \
  --memory 6g --cpus 2 --cpuset-cpus 6,7 --pids-limit 256 \
  --mount type=bind,src="$MODELS",dst=/models,readonly \
  --mount type=bind,src="$ROOT/tools",dst=/tools,readonly \
  --mount type=bind,src="$ROOT/schemas",dst=/schemas,readonly \
  --mount type=bind,src="$OUT",dst=/out \
  mimik-laya:0.4.15 python3 -I /tools/laya_eval.py /models /out/laya.json </dev/null
