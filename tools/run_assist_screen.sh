#!/usr/bin/env bash
# Step-2b interpretation screen runner (prototype tier; criteria in docs/assisted-sheet-plan.md
# and the results in docs/results.md). Sequential, unattended, never interactive.
#
# Usage: tools/run_assist_screen.sh <out-dir> <model-key>...
# For each registry model: hash check (--network none), 3 cold starts (start -> /health, then
# one request with an empty prompt cache), then the warm screen (2 passes + 4 stress requests)
# on the third server. llama-server runs on an internal network with no published port, CPU
# only, pinned to CPUS (default 0,1) with 2 threads; the harness runs on HARNESS_CPU (7).
# Environment: DEADLINE_EPOCH (default now + 10 h): no request starts after it.
# Small CPU models at 2 cores are allowed at any hour (AGENTS.md), so no weekday window applies.
set -euo pipefail
export TZ=:/etc/localtime
ROOT=$(cd "$(dirname "$0")/.." && pwd)
OUT=$1; shift
CPUS=${CPUS:-0,1}
HARNESS_CPU=${HARNESS_CPU:-7}
DEADLINE_EPOCH=${DEADLINE_EPOCH:-$(( $(date +%s) + 36000 ))}
CHECK_IMAGE=sha256:9578747a34f5e02cb7561c8e2a8903b75100b96ce1b0587d5fa4b687be0d3a9f
HARNESS_IMAGE=sha256:8477ff915533b4a21a9727d2804acd06bae15b93e95d60627f8e127cd7025a7d
NET=mimik-assist2b-net
SRV=mimik-assist2b-srv
HARNESS=mimik-assist2b-harness
LABEL=mimik.assist2b=screen
mkdir -p "$OUT"
log() { echo "$(date '+%F %T') $*" | tee -a "$OUT/run.log"; }
cleanup() { docker rm -f "$SRV" "$HARNESS" >/dev/null 2>&1 || true; }
trap cleanup EXIT
if ! docker network inspect "$NET" >/dev/null 2>&1; then
  docker network create --internal --label "$LABEL" "$NET" >/dev/null
fi
[ "$(docker network inspect -f '{{.Internal}}' "$NET")" = true ] || { log "network not internal"; exit 2; }

reg() { python3 "$ROOT/tools/model_registry.py" get "$1" | python3 -c "import json,sys;print(json.load(sys.stdin)$2)"; }

start_server() {  # start_server <model-file>; echoes seconds to /health
  local model=$1 t0 now
  docker rm -f "$SRV" >/dev/null 2>&1 || true
  t0=$(date +%s.%N)
  docker run -d --init --pull never --name "$SRV" --label "$LABEL" --network "$NET" \
    --network-alias server --user 1000:1000 --cap-drop ALL --security-opt no-new-privileges \
    --read-only --tmpfs /tmp --memory 6g --memory-swap 6g --cpus 2 --cpuset-cpus "$CPUS" \
    --pids-limit 256 --mount type=bind,src="$model",dst=/models/model.gguf,readonly \
    "$IMAGE" "${ARGS[@]}" >/dev/null
  for _ in $(seq 1 1800); do
    if docker exec "$SRV" curl -sf http://127.0.0.1:8080/health >/dev/null 2>&1; then
      now=$(date +%s.%N); python3 -c "print(round($now - $t0, 3))"; return 0
    fi
    [ "$(docker inspect -f '{{.State.Running}}' "$SRV")" = true ] || { log "server exited"; return 1; }
    sleep 0.5
  done
  log "health timeout"; return 1
}

harness() {  # harness <ledger name> <args...>
  local name=$1; shift
  docker rm -f "$HARNESS" >/dev/null 2>&1 || true
  docker run --rm --init --pull never --name "$HARNESS" --label "$LABEL" --network "$NET" \
    --user 1000:1000 --cap-drop ALL --security-opt no-new-privileges --read-only --tmpfs /tmp \
    --memory 1g --cpus 1 --cpuset-cpus "$HARNESS_CPU" --pids-limit 64 \
    -e PYTHONDONTWRITEBYTECODE=1 \
    --mount type=bind,src="$ROOT/tools",dst=/tools,readonly \
    --mount type=bind,src="$OUT",dst=/out \
    "$HARNESS_IMAGE" python3 /tools/assist_eval.py run-llm --endpoint http://server:8080/v1/chat/completions \
    --model-label "$LABEL_M" --model-sha256 "$SHA" --server-args "${ARGS[*]}" \
    --deadline-epoch "$DEADLINE_EPOCH" --output "/out/$name" "$@" >> "$OUT/harness.log" 2>&1
}

for key in "$@"; do
  [ "$(date +%s)" -lt "$DEADLINE_EPOCH" ] || { log "deadline reached before $key"; exit 0; }
  model=$(realpath -e "/media/data/mimik/models/$(reg "$key" "['path']")")
  SHA=$(reg "$key" "['sha256']"); LABEL_M=$(reg "$key" "['label']")
  IMAGE=$(reg "$key" "['server_image']")
  mapfile -t ARGS < <(cd "$ROOT/tools" && python3 -c "
import model_registry as m
d = m.load(); e = m.model(d, '$key')
for a in m.server_args(e, 8192, device='cpu', threads=2): print(a)
print('--threads-batch'); print('2'); print('--reasoning'); print('off')")
  log "$key: hashing $model"
  actual=$(docker run --rm --network none --user 1000:1000 --cap-drop ALL --read-only --memory 256m \
    --mount type=bind,src="$model",dst=/m.gguf,readonly "$CHECK_IMAGE" sha256sum /m.gguf | cut -d' ' -f1)
  [ "$actual" = "$SHA" ] || { log "$key: sha256 mismatch $actual"; exit 5; }
  log "$key: sha256 ok; server args: ${ARGS[*]}"
  mkdir -p "$OUT/$key"
  for i in 1 2 3; do
    health=$(start_server "$model") || { log "$key: cold start $i failed"; continue 2; }
    log "$key: cold start $i health_seconds=$health"
    echo "{\"key\": \"$key\", \"cold_start\": $i, \"health_seconds\": $health}" >> "$OUT/$key/cold.jsonl"
    harness "$key/cold-$i.jsonl" --passes 1 --only dh-01 || log "$key: cold request $i failed"
    if [ "$i" -lt 3 ]; then docker rm -f "$SRV" >/dev/null; fi
  done
  log "$key: warm screen"
  harness "$key/screen.jsonl" --passes 2 --stress 4 || log "$key: harness exit $?"
  docker logs "$SRV" > "$OUT/$key/server.log" 2>&1 || true
  docker rm -f "$SRV" >/dev/null
  log "$key: done"
done
log "all done"
