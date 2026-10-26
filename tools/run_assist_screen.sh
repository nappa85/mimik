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
#   MAX_TOKENS (384), VARIANT (exploratory prompt variant, empty = pre-registered prefix),
#   PRIME (0): 1 sends the priming request (full fixed prefix, max_tokens 1) after every start,
#   before any case, and records it as prime-<n>.jsonl.
#   CONTRACT (v1): v2 is output schema v2 (step 2c-1, aliases), read from schemas/.
#   DEVICE (cpu): gpu passes /dev/dri to the server and offloads all layers (Vulkan). A GPU run
#   requires the fan in manual mode at pwm1 >= 230 and runs a thermal guard that removes the
#   containers and stops the run at junction >= 90 C, edge >= 80 C, or the fan leaving
#   manual-high; the weekday 09:00-18:00 window is the caller's DEADLINE_EPOCH to set.
# The model network is internal with inhibit_ipv4 (no address on the host side); an existing
# network without both is refused.
#   CASES (default: the 2b cases), PASSES (2), STRESS (4), COLD (3 cold starts; each runs the
#   dh-01 request unless COLD_ONLY is empty), THREADS (2: server threads and cpu limit; CPUS
#   must list at least that many cores), NAME (assist2b: container name stem).
# Small CPU models at 2 cores are allowed at any hour (AGENTS.md), so no weekday window applies
# here; tools/run_fieldref.sh adds its own window and idle checks.
set -euo pipefail
export TZ=:/etc/localtime
ROOT=$(cd "$(dirname "$0")/.." && pwd)
OUT=$1; shift
CPUS=${CPUS:-0,1}
HARNESS_CPU=${HARNESS_CPU:-7}
DEADLINE_EPOCH=${DEADLINE_EPOCH:-$(( $(date +%s) + 36000 ))}
MAX_TOKENS=${MAX_TOKENS:-384}
VARIANT=${VARIANT:-}
PRIME=${PRIME:-0}
DEVICE=${DEVICE:-cpu}
CONTRACT=${CONTRACT:-v1}
CASES=${CASES:-assist-eval/cases.json}
PASSES=${PASSES:-2}
STRESS=${STRESS:-4}
COLD=${COLD:-3}
COLD_ONLY=${COLD_ONLY-dh-01}
THREADS=${THREADS:-2}
NAME=${NAME:-assist2b}
[ "$DEVICE" = cpu ] || [ "$DEVICE" = gpu ] || { echo "DEVICE must be cpu or gpu"; exit 2; }
EXTRA=()
[ -n "$VARIANT" ] && EXTRA=(--variant "$VARIANT")
EXTRA+=(--contract "$CONTRACT" --cases "/tools/$CASES")
CHECK_IMAGE=sha256:9578747a34f5e02cb7561c8e2a8903b75100b96ce1b0587d5fa4b687be0d3a9f
HARNESS_IMAGE=sha256:8477ff915533b4a21a9727d2804acd06bae15b93e95d60627f8e127cd7025a7d
NET=mimik-assist2b-net
SRV=mimik-$NAME-srv
HARNESS=mimik-$NAME-harness
LABEL=mimik.assist2b=screen
mkdir -p "$OUT"
log() { echo "$(date '+%F %T') $*" | tee -a "$OUT/run.log"; }
cleanup() { docker rm -f "$SRV" "$HARNESS" >/dev/null 2>&1 || true; }
trap cleanup EXIT
GPU_RUN=()
if [ "$DEVICE" = gpu ]; then
  HW=$(dirname "$(grep -l amdgpu /sys/class/hwmon/hwmon*/name)")
  fan_ok() { [ "$(cat "$HW/pwm1_enable")" = 1 ] && [ "$(cat "$HW/pwm1")" -ge 230 ]; }
  fan_ok || { log "GPU fan not manual at pwm1 >= 230; refusing"; exit 3; }
  GPU_RUN=(--device /dev/dri/renderD128 --device /dev/dri/card0
    --group-add "$(stat -c %g /dev/dri/renderD128)" --group-add "$(stat -c %g /dev/dri/card0)")
  STOP="$OUT/thermal-stop"; rm -f "$STOP"
  ( while :; do
      j=$(( $(cat "$HW/temp2_input" 2>/dev/null || echo 999000) / 1000 ))
      e=$(( $(cat "$HW/temp1_input" 2>/dev/null || echo 999000) / 1000 ))
      reason=""
      [ "$j" -ge 90 ] && reason="junction ${j} C"
      [ "$e" -ge 80 ] && reason="${reason:+$reason; }edge ${e} C"
      fan_ok || reason="${reason:+$reason; }fan left manual-high"
      if [ -n "$reason" ]; then
        echo "$(date '+%F %T') thermal stop: $reason" | tee -a "$OUT/run.log" > "$STOP"
        docker rm -f "$SRV" "$HARNESS" >/dev/null 2>&1; exit 0
      fi
      echo "$(date '+%F %T') edge=$e junction=$j" >> "$OUT/temps.log"
      sleep 2
    done ) &
  GUARD=$!
  trap 'kill "$GUARD" 2>/dev/null; cleanup' EXIT
fi
stopped() { [ -n "${STOP:-}" ] && [ -e "$STOP" ]; }
if ! docker network inspect "$NET" >/dev/null 2>&1; then
  docker network create --internal -o com.docker.network.bridge.inhibit_ipv4=true \
    --label "$LABEL" "$NET" >/dev/null
fi
net_state=$(docker network inspect -f '{{.Internal}} {{index .Options "com.docker.network.bridge.inhibit_ipv4"}}' "$NET")
[ "$net_state" = "true true" ] || { log "network $NET is not internal with inhibit_ipv4 ($net_state)"; exit 2; }

reg() { python3 "$ROOT/tools/model_registry.py" get "$1" | python3 -c "import json,sys;print(json.load(sys.stdin)$2)"; }

start_server() {  # start_server <model-file>; echoes seconds to /health
  local model=$1 t0 now
  docker rm -f "$SRV" >/dev/null 2>&1 || true
  t0=$(date +%s.%N)
  docker run -d --init --pull never --name "$SRV" --label "$LABEL" --network "$NET" \
    --network-alias server --user 1000:1000 --cap-drop ALL --security-opt no-new-privileges \
    --read-only --tmpfs /tmp --memory 6g --memory-swap 6g --cpus "$THREADS" --cpuset-cpus "$CPUS" \
    --pids-limit 256 --mount type=bind,src="$model",dst=/models/model.gguf,readonly \
    "${GPU_RUN[@]}" "$IMAGE" "${ARGS[@]}" >/dev/null
  for _ in $(seq 1 1800); do
    if docker exec "$SRV" curl -sf http://127.0.0.1:8080/health >/dev/null 2>&1; then
      now=$(date +%s.%N); python3 -c "print(round($now - $t0, 3))"; return 0
    fi
    [ "$(docker inspect -f '{{.State.Running}}' "$SRV")" = true ] || { log "server exited"; return 1; }
    sleep 0.5
  done
  log "health timeout"; return 1
}

prime() {  # prime <ledger name>: the priming request, not a case
  docker rm -f "$HARNESS" >/dev/null 2>&1 || true
  docker run --rm --init --pull never --name "$HARNESS" --label "$LABEL" --network "$NET" \
    --user 1000:1000 --cap-drop ALL --security-opt no-new-privileges --read-only --tmpfs /tmp \
    --memory 1g --cpus 1 --cpuset-cpus "$HARNESS_CPU" --pids-limit 64 \
    --mount type=bind,src="$ROOT/tools",dst=/tools,readonly \
    --mount type=bind,src="$ROOT/schemas",dst=/schemas,readonly --mount type=bind,src="$OUT",dst=/out \
    "$HARNESS_IMAGE" python3 /tools/assist_eval.py prime --endpoint http://server:8080/v1/chat/completions \
    --output "/out/$1" --deadline-epoch "$DEADLINE_EPOCH" "${EXTRA[@]}" >> "$OUT/harness.log" 2>&1
}

harness() {  # harness <ledger name> <args...>
  local name=$1; shift
  docker rm -f "$HARNESS" >/dev/null 2>&1 || true
  docker run --rm --init --pull never --name "$HARNESS" --label "$LABEL" --network "$NET" \
    --user 1000:1000 --cap-drop ALL --security-opt no-new-privileges --read-only --tmpfs /tmp \
    --memory 1g --cpus 1 --cpuset-cpus "$HARNESS_CPU" --pids-limit 64 \
    -e PYTHONDONTWRITEBYTECODE=1 \
    --mount type=bind,src="$ROOT/tools",dst=/tools,readonly \
    --mount type=bind,src="$ROOT/schemas",dst=/schemas,readonly \
    --mount type=bind,src="$OUT",dst=/out \
    "$HARNESS_IMAGE" python3 /tools/assist_eval.py run-llm --endpoint http://server:8080/v1/chat/completions \
    --model-label "$LABEL_M" --model-sha256 "$SHA" --server-args "${ARGS[*]}" \
    --deadline-epoch "$DEADLINE_EPOCH" --max-tokens "$MAX_TOKENS" --output "/out/$name" \
    "${EXTRA[@]}" "$@" >> "$OUT/harness.log" 2>&1
}

for key in "$@"; do
  [ "$(date +%s)" -lt "$DEADLINE_EPOCH" ] || { log "deadline reached before $key"; exit 0; }
  stopped && { log "thermal stop before $key"; exit 4; }
  model=$(realpath -e "/media/data/mimik/models/$(reg "$key" "['path']")")
  SHA=$(reg "$key" "['sha256']"); LABEL_M=$(reg "$key" "['label']")
  IMAGE=$(reg "$key" "['server_image']")
  mapfile -t ARGS < <(cd "$ROOT/tools" && python3 -c "
import model_registry as m
d = m.load(); e = m.model(d, '$key')
for a in m.server_args(e, 8192, device='$DEVICE', threads=$THREADS): print(a)
print('--threads-batch'); print('$THREADS'); print('--reasoning'); print('off')")
  log "$key: hashing $model"
  actual=$(docker run --rm --network none --user 1000:1000 --cap-drop ALL --read-only --memory 256m \
    --mount type=bind,src="$model",dst=/m.gguf,readonly "$CHECK_IMAGE" sha256sum /m.gguf | cut -d' ' -f1)
  [ "$actual" = "$SHA" ] || { log "$key: sha256 mismatch $actual"; exit 5; }
  log "$key: sha256 ok; server args: ${ARGS[*]}"
  mkdir -p "$OUT/$key"
  for i in $(seq 1 "$COLD"); do
    [ "$(date +%s)" -lt "$DEADLINE_EPOCH" ] || { log "deadline reached before start $i of $key"; exit 0; }
    health=$(start_server "$model") || { log "$key: cold start $i failed"; continue 2; }
    log "$key: cold start $i health_seconds=$health"
    echo "{\"key\": \"$key\", \"cold_start\": $i, \"health_seconds\": $health}" >> "$OUT/$key/cold.jsonl"
    if [ "$PRIME" = 1 ]; then
      prime "$key/prime-$i.jsonl" || { log "$key: priming $i failed"; continue 2; }
      log "$key: primed $i"
    fi
    if [ -n "$COLD_ONLY" ]; then
      harness "$key/cold-$i.jsonl" --passes 1 --only "$COLD_ONLY" || log "$key: cold request $i failed"
    fi
    if [ "$i" -lt "$COLD" ]; then docker rm -f "$SRV" >/dev/null; fi
  done
  stopped && { log "thermal stop during $key cold starts"; exit 4; }
  log "$key: warm screen"
  harness "$key/screen.jsonl" --passes "$PASSES" --stress "$STRESS" || log "$key: harness exit $?"
  docker logs "$SRV" > "$OUT/$key/server.log" 2>&1 || true
  docker rm -f "$SRV" >/dev/null
  stopped && { log "thermal stop during $key"; exit 4; }
  log "$key: done"
done
log "all done"
