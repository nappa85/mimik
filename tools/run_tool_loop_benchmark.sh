#!/usr/bin/env bash
# Canonical live runner for tools/tool_loop_benchmark.py. Prototype tier. Written for harness
# v2; with harness v3 it runs the C0 default flags (no v3 pass-through yet, plan §11 item 26),
# and the held-out split is an empty placeholder that the harness refuses to run.
#
# Usage: tools/run_tool_loop_benchmark.sh <model-key> <split> [seeds] [ctx]
#   model-key: qwen35-9b | qwen3-4b-2507 | qwen36-35b-a3b (needs N_CPU_MOE)
#   split:     smoke (dev-001, dev-005, dev-008) | dev | heldout
#   seeds:     default 17,29        ctx: default 16384
# Environment:
#   BUDGET_SECONDS  harness run budget (default 0 = none)
#   N_CPU_MOE       llama.cpp --n-cpu-moe (required for qwen36-35b-a3b, MoE experts in RAM)
#   SRV_MEMORY      server container memory limit (default 14g; 26g for qwen36-35b-a3b)
#   MODEL_PATH / MODEL_SHA256      override the model file and its expected sha256
#   FORMULA_CLI_SHA256             expected sha256 of the formula_eval CLI (default: pinned)
#   CLI_BUILD_INFO  key=value file with source_commit= for the CLI build
#   CHECK_ONLY=1    stop after the pre-launch hash checks and provenance
#
# Runs are strictly sequential (one GPU): a lock file refuses a second concurrent run.
# Before launch, the model and the formula CLI are hashed in a locked-down container and the
# run aborts on any mismatch. The server and harness run on an internal Docker network with no
# published ports, as uid 1000, with all capabilities dropped and read-only root filesystems.
# The harness itself fails fast (5 consecutive error episodes, failed health check, formula CLI
# fault). Output: target/tool-loop-benchmark-<date>/<model-key>/<split>-<time>/.
# Exit codes: 2 usage, 3 CLI hash, 4 server not healthy, 5 model hash, 9 another run active;
# otherwise the harness exit code (3 formula infrastructure fault, 4 fail-fast abort).
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
RUN_BASE=$ROOT/target/tool-loop-benchmark-$(date +%Y%m%d)
IMAGE=ghcr.io/ggml-org/llama.cpp@sha256:217d278eded30ca7b61321f4ce4dd86a0a56157f54b73e5600e2ad46070ddfff
# Local image ids, pinned because the tags may move: mimik-ci (hash checks) and
# mimik-toolloop-py:20261015 (FROM mimik-ci plus chmod 0711 /root so uid 1000 reaches python).
CHECK_IMAGE=sha256:9578747a34f5e02cb7561c8e2a8903b75100b96ce1b0587d5fa4b687be0d3a9f
HARNESS_IMAGE=sha256:8477ff915533b4a21a9727d2804acd06bae15b93e95d60627f8e127cd7025a7d
NET=mimik-toolloop-v2
SRV=mimik-toolloop-v2-srv
CLI_IN_VOLUME=harness-formula-eval/release/examples/formula_eval
CLI_SHA=${FORMULA_CLI_SHA256:-f0e13322ef921fa89685c48831e9a7658cf789e842514a8a93ff891fa334fc10}
CLI_BUILD_INFO=${CLI_BUILD_INFO:-$ROOT/target/tool-loop-benchmark-20261015/harness-v2/formula-cli-build.txt}
SMOKE_CASES=dev-001,dev-005,dev-008
[ $# -ge 2 ] || { sed -n '3,9p' "$0" >&2; exit 2; }
key=$1 split=$2 seeds=${3:-17,29} ctx=${4:-16384}
mkdir -p "$ROOT/target"
exec 9>"$ROOT/target/.tool-loop-benchmark.lock"
flock -n 9 || { echo "another tool-loop run is active; runs are strictly sequential" >&2; exit 9; }
srv_memory=14g moe_args=()
case $key in
  qwen35-9b) model=$ROOT/target/model-fetch-20261015/Qwen3.5-9B-Q4_K_M.gguf
             sha=03b74727a860a56338e042c4420bb3f04b2fec5734175f4cb9fa853daf52b7e8
             label=qwen3.5-9b-q4_k_m ;;
  qwen3-4b-2507) model=$ROOT/target/local-rag-benchmark-20261015/Qwen3-4B-Instruct-2507-Q4_K_M.gguf
             sha=2fde00ce69dd4899c70d020845e2638353015bba0fdf161b3eb965f2bca4464e
             label=qwen3-4b-instruct-2507-q4_k_m ;;
  qwen36-35b-a3b) model=$ROOT/target/model-fetch-20261015/Qwen3.6-35B-A3B-Q4_K_M.gguf
             sha=671e47e0ec53c665d048b98c3ecbfd5236b5ca9c3e02ed19fc8f81f7b85140c7
             label=qwen3.6-35b-a3b-q4_k_m srv_memory=26g
             [ -n "${N_CPU_MOE:-}" ] || { echo "qwen36-35b-a3b needs N_CPU_MOE (llama.cpp --n-cpu-moe)" >&2; exit 2; }
             [[ $N_CPU_MOE =~ ^[0-9]+$ ]] || { echo "N_CPU_MOE must be a non-negative integer" >&2; exit 2; } ;;
  *) echo "unknown model $key" >&2; exit 2 ;;
esac
if [ -n "${N_CPU_MOE:-}" ]; then moe_args=(--n-cpu-moe "$N_CPU_MOE"); fi
srv_memory=${SRV_MEMORY:-$srv_memory}
model=${MODEL_PATH:-$model} sha=${MODEL_SHA256:-$sha}
case $split in
  smoke) hsplit=dev case_args=(--cases "$SMOKE_CASES") ;;
  dev|heldout) hsplit=$split case_args=() ;;
  *) echo "unknown split $split" >&2; exit 2 ;;
esac
pack=$ROOT/examples/tool-loop-demo
out=$RUN_BASE/$key/$split-$(date +%Y%m%dT%H%M%S)
mkdir -p "$out"
budget=${BUDGET_SECONDS:-0}
check=(docker run --rm --network none --user 1000:1000 --cap-drop ALL
  --security-opt no-new-privileges --read-only --memory 256m --pids-limit 32)
actual_cli_sha=$("${check[@]}" --mount type=volume,src=mimik-formula-target,dst=/formula,readonly \
  "$CHECK_IMAGE" sha256sum "/formula/$CLI_IN_VOLUME" | cut -d' ' -f1)
if [ "$actual_cli_sha" != "$CLI_SHA" ]; then
  echo "formula_eval sha256 $actual_cli_sha != pinned $CLI_SHA" | tee "$out/not-launched.txt"; exit 3
fi
actual_model_sha=$("${check[@]}" --mount type=bind,src="$model",dst=/models/model.gguf,readonly \
  "$CHECK_IMAGE" sha256sum /models/model.gguf | cut -d' ' -f1)
if [ "$actual_model_sha" != "$sha" ]; then
  echo "model sha256 $actual_model_sha != expected $sha" | tee "$out/not-launched.txt"; exit 5
fi
harness_sha=$(sha256sum "$ROOT/tools/tool_loop_benchmark.py" | cut -d' ' -f1)
git_head=$(git -C "$ROOT" rev-parse HEAD)
git_dirty=$(git -C "$ROOT" status --porcelain | wc -l)
harness_git_state=$(git -C "$ROOT" status --porcelain -- tools/tool_loop_benchmark.py | cut -c1-2)
cli_source_commit=unknown
if [ -f "$CLI_BUILD_INFO" ]; then
  cli_source_commit=$(sed -n 's/^source_commit=\([0-9a-f]*\).*/\1/p' "$CLI_BUILD_INFO")
fi
srv_args=(--model /models/model.gguf --alias "$label" --host 0.0.0.0 --port 8080 --offline
  --no-webui --no-slots --no-mmproj --jinja --n-gpu-layers 99 --fit off --threads 6
  --ctx-size "$ctx" --parallel 1 --no-warmup --cache-ram 0 "${moe_args[@]}")
{
  echo "date_start=$(date -Is)"
  echo "harness=v2 (tools/tool_loop_benchmark.py BENCHMARK_VERSION 2) runner=tools/run_tool_loop_benchmark.sh"
  echo "image=$IMAGE"
  docker image inspect "$IMAGE" --format 'image_id={{.Id}} version={{index .Config.Labels "org.opencontainers.image.version"}} revision={{index .Config.Labels "org.opencontainers.image.revision"}}'
  echo "model=$model sha256=$actual_model_sha (expected $sha, hashed before launch)"
  echo "formula_cli=mimik-formula-target:/$CLI_IN_VOLUME sha256=$actual_cli_sha (pinned $CLI_SHA) source_commit=$cli_source_commit ($CLI_BUILD_INFO)"
  echo "harness_sha256=$harness_sha git_head=$git_head git_dirty_paths=$git_dirty harness_file_git_status='${harness_git_state:-clean}'"
  echo "check_image=$CHECK_IMAGE harness_image=$HARNESS_IMAGE server_memory=$srv_memory"
  echo "server_args=${srv_args[*]}"
  echo "prompt_cache=on (server: no --no-cache-prompt; harness: --cache-prompt; --cache-ram 0 = no extra host-RAM cache)"
} > "$out/provenance.txt"
if [ "${CHECK_ONLY:-0}" = 1 ]; then
  echo "check_only=1 (stopped before launch)" >> "$out/provenance.txt"; exit 0
fi
docker network inspect $NET >/dev/null 2>&1 || docker network create --internal $NET >/dev/null
docker rm -f $SRV >/dev/null 2>&1 || true
docker run -d --name $SRV --network $NET --network-alias server \
  --user 1000:1000 --cap-drop ALL --security-opt no-new-privileges --read-only --tmpfs /tmp \
  --memory "$srv_memory" --memory-swap "$srv_memory" --cpus 6 --pids-limit 512 \
  --device /dev/dri/renderD128 --device /dev/dri/card0 \
  --group-add "$(stat -c %g /dev/dri/renderD128)" --group-add "$(stat -c %g /dev/dri/card0)" \
  --mount type=bind,src="$model",dst=/models/model.gguf,readonly \
  "$IMAGE" "${srv_args[@]}" >/dev/null
cleanup() {
  docker logs $SRV > "$out/server.log" 2>&1 || true
  docker stop -t 5 $SRV >/dev/null 2>&1 || true
  docker rm $SRV >/dev/null 2>&1 || true
  docker network rm $NET >/dev/null 2>&1 || true
  echo "date_end=$(date -Is)" >> "$out/provenance.txt"
}
trap cleanup EXIT
t0=$(date +%s)
for _ in $(seq 1 450); do
  if docker exec $SRV curl -sf http://127.0.0.1:8080/health >/dev/null 2>&1; then break; fi
  if ! docker ps -q -f name=^/$SRV$ | grep -q .; then echo "server exited during load" | tee -a "$out/provenance.txt"; exit 4; fi
  sleep 2
done
docker exec $SRV curl -sf http://127.0.0.1:8080/health >/dev/null || { echo "server not healthy" | tee -a "$out/provenance.txt"; exit 4; }
echo "server_ready_seconds=$(( $(date +%s) - t0 ))" >> "$out/provenance.txt"
docker exec $SRV curl -sf http://127.0.0.1:8080/props > "$out/props.json" || true
ledger=/out/ledger-$(date +%Y%m%dT%H%M%S).jsonl
cli=(python3 /tools/tool_loop_benchmark.py --pack /pack --split "$hsplit" "${case_args[@]}"
  --endpoint http://server:8080
  --model-label "$label" --seeds "$seeds" --temperature 0.1 --thinking off --cache-prompt
  --tool-choice auto --request-timeout 180 --budget-seconds "$budget"
  --formula-cli "/formula/$CLI_IN_VOLUME" --model-sha256 "$actual_model_sha"
  --provenance "git_head=$git_head" --provenance "git_dirty_paths=$git_dirty"
  --provenance "harness_file_git_status=${harness_git_state:-clean}"
  --provenance "cli_source_commit=$cli_source_commit" --provenance "server_image=$IMAGE"
  --provenance "n_cpu_moe=${N_CPU_MOE:-}" --output "$ledger")
echo "harness_cli=${cli[*]}" >> "$out/provenance.txt"
echo "pack=$pack" >> "$out/provenance.txt"
h0=$(date +%s)
set +e
docker run --rm --name mimik-toolloop-v2-harness --network $NET \
  --user 1000:1000 --cap-drop ALL --security-opt no-new-privileges --read-only --tmpfs /tmp \
  --memory 2g --cpus 2 --pids-limit 128 -e PYTHONDONTWRITEBYTECODE=1 \
  --mount type=bind,src="$ROOT/tools",dst=/tools,readonly \
  --mount type=bind,src="$pack",dst=/pack,readonly \
  --mount type=volume,src=mimik-formula-target,dst=/formula,readonly \
  --mount type=bind,src="$out",dst=/out \
  "$HARNESS_IMAGE" "${cli[@]}" > "$out/harness.stdout" 2> "$out/harness.stderr"
rc=$?
set -e
echo "harness_exit=$rc harness_wall_seconds=$(( $(date +%s) - h0 ))" >> "$out/provenance.txt"
exit $rc
