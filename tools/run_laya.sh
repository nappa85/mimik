#!/usr/bin/env bash
# Laya prototype run (decision rule: tools/fieldref-prereg.md; run only after the owner has
# confirmed the review). CPU, 2 threads on cores 6,7, no network, models read-only.
# Usage: tools/run_laya.sh [smoke|all] [out-dir]   (default out: /media/data/mimik/laya/runs/<stamp>)
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
MODELS=/media/data/mimik/laya/laya-7b928d82
MODE=${1:-all}
OUT=${2:-/media/data/mimik/laya/runs/$(date +%Y%m%d-%H%M)}
# Image mimik-laya:0.4.15 by id; per-file model hashes are pinned in the repo.
IMAGE=sha256:0383e06f890ac9db2c197c6a6e391a3207e14f43d189566e500ff93498ee35aa
(cd "$MODELS" && sha256sum -c --quiet "$ROOT/tools/laya-models.sha256")
mkdir -p "$OUT"
docker run --rm --init --pull never --name mimik-laya-run --network none --user 1000:1000 \
  --cap-drop ALL --security-opt no-new-privileges --read-only --tmpfs /tmp -e HF_HOME=/tmp \
  --memory 6g --cpus 2 --cpuset-cpus 6,7 --pids-limit 256 \
  --mount type=bind,src="$MODELS",dst=/models,readonly \
  --mount type=bind,src="$ROOT/tools",dst=/tools,readonly \
  --mount type=bind,src="$ROOT/schemas",dst=/schemas,readonly \
  --mount type=bind,src="$OUT",dst=/out \
  "$IMAGE" python3 -I /tools/laya_eval.py /models "/out/laya-$MODE.json" "$MODE" </dev/null
