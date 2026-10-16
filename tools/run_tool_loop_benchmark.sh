#!/usr/bin/env bash
# Canonical live runner for tools/tool_loop_benchmark.py (harness v3). Prototype tier.
# Models come from the registry tools/models.json (tools/model_registry.py): a new model is a
# registry entry, never a change here. The held-out split is an empty placeholder until the
# blind writer delivers it; the harness refuses to run it.
#
# Usage: tools/run_tool_loop_benchmark.sh <model-key> <split> [seeds] [ctx]
#   model-key: a key of tools/models.json (python3 tools/model_registry.py list)
#   split:     smoke (dev-001, dev-005, dev-008) | dev | heldout
#   seeds:     default $SEEDS or 17,29      ctx: default $CTX or the registry ctx
# Environment (unset = the C0 base of plan §11 item 5, or the registry value):
#   ARM             arm label (ledger header and every episode)
#   CTX SEEDS       as the positional arguments (a positional value wins)
#   MAX_TURNS (8) MAX_TOKENS (4096) THINKING (off|on|default) TEMPERATURE (0.1) TOP_P (0.95)
#   REQUEST_TIMEOUT HEALTH_WAIT SRV_MEMORY   (default: registry)
#   NUDGE (1) TURNS_REMAINING (0) SEARCH_NOTES (0) SANITIZE_ECHO (1) PENDING_CURRENT (1)
#   UNTRUSTED (0)   harness flags H1, H2a, H2b, H3, H5, H6 as 0|1
#   CASES           comma-separated case ids;  CASES_FILE  hashed subset (tools/case-subsets/)
#   QUIET=1         harness --quiet-progress (no per-case outcome on stderr; plan §11 item 3)
#   PREFLIGHT_CASE  dev case id: 1-case, seed-17 dev run on the same server before the arm
#   BUDGET_SECONDS  harness run budget (default: seconds left in the schedule window - 600)
#   N_CPU_MOE       llama.cpp --n-cpu-moe (required where the registry says so)
#   MIMIK_MODEL_ROOT  model root (default: registry model_root)
#   MODEL_PATH / MODEL_SHA256      override the model file and its expected sha256
#   HASH_CACHE      file of model hashes verified this session (the queue sets it)
#   OUT_DIR         output directory (default target/tool-loop-benchmark-<date>/<key>/...)
#   LOCK_FILE       default <main checkout>/target/.tool-loop-benchmark.lock (all worktrees)
#   FORMULA_CLI_SHA256 / CLI_BUILD_INFO   pinned formula_eval CLI and its build record
#   CHECK_ONLY=1    stop after the pre-launch checks and provenance (no model load)
#   SCHEDULE_OVERRIDE=user-approved   heavy work inside the weekday 09:00-18:00 block
#
# Runs are strictly sequential (one GPU): a lock file refuses a second concurrent run. Heavy
# work is refused Monday-Friday 09:00-18:00 local time (AGENTS.md machine schedule), and the
# harness budget ends before the window closes; tools/run_queue.py also stops a running arm at
# the window edge. The model path is resolved (realpath); the resolved file is size-checked,
# hashed in a locked-down container (or matched in HASH_CACHE), pre-warmed into the page cache
# when the registry says so (plan §11 item 27), and bind-mounted read-only. Server and harness
# run on an internal Docker network with no published ports and never pull images, as uid
# 1000, all capabilities dropped, read-only root filesystems. The harness fails fast (5
# consecutive error episodes, failed health check, formula CLI fault). provenance.txt holds
# key=value lines, with failure=<class> on any runner failure (tools/run_queue.py reads it).
# Exit codes: 2 usage/config, 3 CLI hash, 4 server start/health, 5 model file/hash, 6 preflight,
# 9 another run active, 10 outside the schedule window, 130/143 signal; otherwise the harness
# exit code (3 formula infrastructure fault, 4 fail-fast abort).
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
MAIN=$(dirname "$(git -C "$ROOT" rev-parse --path-format=absolute --git-common-dir)")
RUN_BASE=$ROOT/target/tool-loop-benchmark-$(date +%Y%m%d)
# Local image ids, pinned because the tags may move: mimik-ci (hash checks) and
# mimik-toolloop-py:20261015 (FROM mimik-ci plus chmod 0711 /root so uid 1000 reaches python).
CHECK_IMAGE=sha256:9578747a34f5e02cb7561c8e2a8903b75100b96ce1b0587d5fa4b687be0d3a9f
HARNESS_IMAGE=sha256:8477ff915533b4a21a9727d2804acd06bae15b93e95d60627f8e127cd7025a7d
NET=mimik-toolloop-v3
SRV=mimik-toolloop-v3-srv
HARNESS=mimik-toolloop-v3-harness
CLI_IN_VOLUME=harness-formula-eval/release/examples/formula_eval
CLI_SHA=${FORMULA_CLI_SHA256:-f0e13322ef921fa89685c48831e9a7658cf789e842514a8a93ff891fa334fc10}
CLI_BUILD_INFO=${CLI_BUILD_INFO:-$MAIN/target/tool-loop-benchmark-20261015/harness-v2/formula-cli-build.txt}
SMOKE_CASES=dev-001,dev-005,dev-008
WINDOW_MARGIN=600
[ $# -ge 2 ] || { sed -n '7,10p' "$0" >&2; exit 2; }
key=$1 split=$2 seeds=${3:-${SEEDS:-17,29}}
exec </dev/null
die() { echo "$2" >&2; exit "$1"; }
is_uint() { [[ $1 =~ ^[0-9]+$ ]]; }
lock=${LOCK_FILE:-$MAIN/target/.tool-loop-benchmark.lock}
mkdir -p "$(dirname "$lock")"
exec 9>"$lock"
flock -n 9 || die 9 "another tool-loop run is active; runs are strictly sequential"

# ---- configuration (registry + pass-through), validated before anything runs
[ -z "${N_CPU_MOE:-}" ] || is_uint "$N_CPU_MOE" || die 2 "N_CPU_MOE must be a non-negative integer"
ctx_arg=${4:-${CTX:-}}
[ -z "$ctx_arg" ] || is_uint "$ctx_arg" || die 2 "ctx must be a positive integer"
reg_cmd=(python3 "$ROOT/tools/model_registry.py" shell "$key")
[ -z "$ctx_arg" ] || reg_cmd+=(--ctx "$ctx_arg")
[ -z "${N_CPU_MOE:-}" ] || reg_cmd+=(--n-cpu-moe "$N_CPU_MOE")
# Declared here so the names are visible; the registry tool assigns them.
reg_label=''
reg_path=''
reg_sha256=''
reg_size=''
reg_image=''
reg_ctx=''
reg_n_cpu_moe=''
reg_memory=''
reg_prewarm=''
reg_health_wait=''
reg_request_timeout=''
reg_thinking=''
reg_tools=''
reg_server_config=''
reg_server_args=()
reg=$("${reg_cmd[@]}") || exit 2
# The registry tool prints only shlex-quoted assignments (tools/test_model_registry.py).
eval "$reg"
ctx=$reg_ctx
srv_memory=${SRV_MEMORY:-$reg_memory}
health_wait=${HEALTH_WAIT:-$reg_health_wait}
request_timeout=${REQUEST_TIMEOUT:-$reg_request_timeout}
max_turns=${MAX_TURNS:-8} max_tokens=${MAX_TOKENS:-4096} thinking=${THINKING:-off}
temperature=${TEMPERATURE:-0.1} top_p=${TOP_P:-0.95}
for v in health_wait max_turns max_tokens; do is_uint "${!v}" || die 2 "$v must be an integer"; done
[[ $srv_memory =~ ^[1-9][0-9]*[mg]$ ]] || die 2 "SRV_MEMORY must look like 14g"
for v in request_timeout temperature top_p; do
  [[ ${!v} =~ ^[0-9]+([.][0-9]+)?$ ]] || die 2 "$v must be a non-negative number"
done
case $thinking in off|on|default) ;; *) die 2 "THINKING must be off, on or default" ;; esac
flags=()
for spec in NUDGE:nudge:1 TURNS_REMAINING:turns-remaining:0 SEARCH_NOTES:search-notes:0 \
  SANITIZE_ECHO:sanitize-echo:1 PENDING_CURRENT:pending-current:1 UNTRUSTED:untrusted-marking:0; do
  IFS=: read -r var name default <<< "$spec"
  case ${!var:-$default} in
    1) flags+=("--$name") ;;
    0) flags+=("--no-$name") ;;
    *) die 2 "$var must be 0 or 1" ;;
  esac
done
[ -z "${CASES:-}" ] || [ -z "${CASES_FILE:-}" ] || die 2 "CASES and CASES_FILE are exclusive"
case $split in
  smoke) hsplit=dev case_args=(--cases "$SMOKE_CASES")
         [ -z "${CASES:-}${CASES_FILE:-}" ] || die 2 "smoke has a fixed case list" ;;
  dev|heldout) hsplit=$split case_args=() ;;
  *) die 2 "unknown split $split" ;;
esac
[ -z "${CASES:-}" ] || case_args=(--cases "$CASES")
[ -z "${BUDGET_SECONDS:-}" ] || is_uint "$BUDGET_SECONDS" || die 2 "BUDGET_SECONDS must be an integer"
model=${MODEL_PATH:-$reg_path} sha=${MODEL_SHA256:-$reg_sha256}
pack=$ROOT/examples/tool-loop-demo
out=${OUT_DIR:-$RUN_BASE/$key/$split-$(date +%Y%m%dT%H%M%S)}
mkdir -p "$out"
prov=$out/provenance.txt
fail() {  # fail <exit> <class> <message>
  echo "$3" >&2
  { echo "failure=$2"; echo "failure_detail=$3"; } >> "$prov"
  exit "$1"
}
{
  echo "date_start=$(date -Is)"
  echo "runner=tools/run_tool_loop_benchmark.sh harness=v3 key=$key split=$split arm=${ARM:-}"
} > "$prov"
cases_file_sha=
if [ -n "${CASES_FILE:-}" ]; then
  [ -f "$CASES_FILE" ] || fail 2 config "CASES_FILE $CASES_FILE not found"
  cp -- "$CASES_FILE" "$out/cases-file.json"
  cases_file_sha=$(sha256sum "$out/cases-file.json" | cut -d' ' -f1)
  case_args=(--cases-file /out/cases-file.json)
fi

# ---- schedule window (AGENTS.md): heavy work only outside weekday 09:00-18:00
budget=${BUDGET_SECONDS:-0}
if [ "${CHECK_ONLY:-0}" != 1 ]; then
  if [ "${SCHEDULE_OVERRIDE:-}" = user-approved ]; then
    echo "schedule_override=user-approved" >> "$prov"
  else
    window=$(python3 "$ROOT/tools/run_queue.py" window-check) \
      || fail 10 window "outside the heavy-work window: $window"
    echo "$window" >> "$prov"
    left=$(sed -n 's/.*seconds_left=\([0-9]*\).*/\1/p' <<< "$window")
    [ -n "$left" ] && [ "$left" -gt "$WINDOW_MARGIN" ] \
      || fail 10 window "heavy-work window closes in ${left:-?}s"
    left=$(( left - WINDOW_MARGIN ))
    if [ "$budget" = 0 ] || [ "$budget" -gt "$left" ]; then
      budget=$left; echo "budget_from_window=$left" >> "$prov"
    fi
  fi
fi

# ---- model file: resolve symlinks, size, hash (or session cache)
model_real=$(realpath -e -- "$model" 2>/dev/null) || fail 5 model_missing "model file $model not found"
[ -f "$model_real" ] || fail 5 model_missing "model $model_real is not a regular file"
size=$(stat -c %s "$model_real")
if [ -z "${MODEL_PATH:-}" ] && [ "$size" != "$reg_size" ]; then
  fail 5 model_size "model size $size != registry $reg_size ($model_real)"
fi
check=(docker run --rm --pull never --network none --user 1000:1000 --cap-drop ALL
  --security-opt no-new-privileges --read-only --memory 256m --pids-limit 32)
actual_cli_sha=$("${check[@]}" --mount type=volume,src=mimik-formula-target,dst=/formula,readonly \
  "$CHECK_IMAGE" sha256sum "/formula/$CLI_IN_VOLUME" | cut -d' ' -f1)
if [ "$actual_cli_sha" != "$CLI_SHA" ]; then
  fail 3 cli_hash "formula_eval sha256 $actual_cli_sha != pinned $CLI_SHA"
fi
stamp="$model_real $(stat -c '%s %Y %i %d' "$model_real")"
hash_source=hashed actual_model_sha=''
if [ -n "${HASH_CACHE:-}" ] && [ -f "$HASH_CACHE" ]; then
  actual_model_sha=$(awk -v s="$stamp" '{h=$NF; $NF=""; sub(/ $/, ""); if ($0 == s) print h}' \
    "$HASH_CACHE" | tail -1)
  [ -z "$actual_model_sha" ] || hash_source=session_cache
fi
h0=$(date +%s)
if [ -z "$actual_model_sha" ]; then
  actual_model_sha=$("${check[@]}" --mount type=bind,src="$model_real",dst=/models/model.gguf,readonly \
    "$CHECK_IMAGE" sha256sum /models/model.gguf | cut -d' ' -f1)
fi
echo "model_hash_seconds=$(( $(date +%s) - h0 )) model_sha256_source=$hash_source" >> "$prov"
if [ "$actual_model_sha" != "$sha" ]; then
  fail 5 model_hash "model sha256 $actual_model_sha != expected $sha"
fi
if [ -n "${HASH_CACHE:-}" ] && [ "$hash_source" = hashed ]; then
  echo "$stamp $actual_model_sha" >> "$HASH_CACHE"
fi
harness_sha=$(sha256sum "$ROOT/tools/tool_loop_benchmark.py" | cut -d' ' -f1)
runner_sha=$(sha256sum "$0" | cut -d' ' -f1)
registry_sha=$(sha256sum "$ROOT/tools/models.json" | cut -d' ' -f1)
git_head=$(git -C "$ROOT" rev-parse HEAD)
git_dirty=$(git -C "$ROOT" status --porcelain | wc -l)
harness_git_state=$(git -C "$ROOT" status --porcelain -- tools/tool_loop_benchmark.py | cut -c1-2)
cli_source_commit=unknown
if [ -f "$CLI_BUILD_INFO" ]; then
  cli_source_commit=$(sed -n 's/^source_commit=\([0-9a-f]*\).*/\1/p' "$CLI_BUILD_INFO")
fi
{
  echo "image=$reg_image"
  docker image inspect "$reg_image" --format 'image_id={{.Id}} version={{index .Config.Labels "org.opencontainers.image.version"}} revision={{index .Config.Labels "org.opencontainers.image.revision"}}' 2>/dev/null \
    || echo "image_id=not-present-locally"
  echo "model=$model resolved=$model_real sha256=$actual_model_sha (expected $sha) size=$size"
  echo "formula_cli=mimik-formula-target:/$CLI_IN_VOLUME sha256=$actual_cli_sha (pinned $CLI_SHA) source_commit=$cli_source_commit ($CLI_BUILD_INFO)"
  echo "harness_sha256=$harness_sha runner_sha256=$runner_sha registry_sha256=$registry_sha"
  echo "git_head=$git_head git_dirty_paths=$git_dirty harness_file_git_status='${harness_git_state:-clean}'"
  echo "check_image=$CHECK_IMAGE harness_image=$HARNESS_IMAGE server_memory=$srv_memory"
  echo "ctx=$ctx thinking=$thinking thinking_capability=$reg_thinking tools=$reg_tools"
  echo "cases_file_sha256=$cases_file_sha budget_seconds=$budget"
  echo "server_args=${reg_server_args[*]}"
  echo "prompt_cache=on (server: no --no-cache-prompt; harness: --cache-prompt; --cache-ram 0 = no extra host-RAM cache)"
} >> "$prov"
if [ "${CHECK_ONLY:-0}" = 1 ]; then
  echo "check_only=1 (stopped before launch)" >> "$prov"; exit 0
fi
if [ "$reg_prewarm" = 1 ]; then  # a cold read of a big GGUF on the HDD outlasts the health wait
  p0=$(date +%s)
  cat -- "$model_real" > /dev/null
  echo "prewarm_seconds=$(( $(date +%s) - p0 ))" >> "$prov"
fi

# ---- server
vram() { cat /sys/class/drm/card0/device/mem_info_vram_used 2>/dev/null || echo n/a; }
swap_used() { free -m | awk '/^Swap:/{print $3}'; }
echo "vram_used_before=$(vram) swap_used_mib_before=$(swap_used)" >> "$prov"
docker network inspect "$NET" >/dev/null 2>&1 || docker network create --internal "$NET" >/dev/null
docker rm -f "$SRV" "$HARNESS" >/dev/null 2>&1 || true
cleanup() {
  local cid peak
  cid=$(docker inspect -f '{{.Id}}' "$SRV" 2>/dev/null || true)
  if [ -n "$cid" ]; then
    peak=$(cat "/sys/fs/cgroup/system.slice/docker-$cid.scope/memory.peak" 2>/dev/null \
      || cat "/sys/fs/cgroup/docker/$cid/memory.peak" 2>/dev/null || echo n/a)
    {
      echo "server_oom_killed=$(docker inspect -f '{{.State.OOMKilled}}' "$SRV" 2>/dev/null || echo unknown)"
      echo "server_memory_peak=$peak vram_used_end=$(vram) swap_used_mib_after=$(swap_used)"
    } >> "$prov"
    docker logs "$SRV" > "$out/server.log" 2>&1 || true
  fi
  docker stop -t 5 "$HARNESS" >/dev/null 2>&1 || true
  docker rm -f "$HARNESS" >/dev/null 2>&1 || true
  docker stop -t 5 "$SRV" >/dev/null 2>&1 || true
  docker rm -f "$SRV" >/dev/null 2>&1 || true
  docker network rm "$NET" >/dev/null 2>&1 || true
  echo "date_end=$(date -Is)" >> "$prov"
}
trap cleanup EXIT
trap 'echo "stopped_by_signal=TERM" >> "$prov"; exit 143' TERM
trap 'echo "stopped_by_signal=INT" >> "$prov"; exit 130' INT
t0=$(date +%s)
docker run -d --pull never --name "$SRV" --network "$NET" --network-alias server \
  --user 1000:1000 --cap-drop ALL --security-opt no-new-privileges --read-only --tmpfs /tmp \
  --memory "$srv_memory" --memory-swap "$srv_memory" --cpus 6 --pids-limit 512 \
  --device /dev/dri/renderD128 --device /dev/dri/card0 \
  --group-add "$(stat -c %g /dev/dri/renderD128)" --group-add "$(stat -c %g /dev/dri/card0)" \
  --mount type=bind,src="$model_real",dst=/models/model.gguf,readonly \
  "$reg_image" "${reg_server_args[@]}" >/dev/null || fail 4 server_start "server container did not start"
healthy=0
while [ $(( $(date +%s) - t0 )) -lt "$health_wait" ]; do
  if docker exec "$SRV" curl -sf http://127.0.0.1:8080/health >/dev/null 2>&1; then healthy=1; break; fi
  docker ps -q -f "name=^/$SRV$" | grep -q . || fail 4 server_start "server exited during load"
  sleep 2
done
[ $healthy = 1 ] || fail 4 server_health "server not healthy after ${health_wait}s"
echo "server_ready_seconds=$(( $(date +%s) - t0 )) vram_used_loaded=$(vram)" >> "$prov"
docker exec "$SRV" curl -sf http://127.0.0.1:8080/props > "$out/props.json" || true

# ---- harness
quiet=()
[ "${QUIET:-0}" = 1 ] && quiet=(--quiet-progress)
common=(python3 /tools/tool_loop_benchmark.py --pack /pack --endpoint http://server:8080
  --model-label "$reg_label" --temperature "$temperature" --top-p "$top_p"
  --max-tokens "$max_tokens" --max-turns "$max_turns" --thinking "$thinking"
  --thinking-capability "$reg_thinking" --cache-prompt --tool-choice auto
  --request-timeout "$request_timeout" --ctx "$ctx" "${flags[@]}" "${quiet[@]}"
  --server-config "$reg_server_config"
  --formula-cli "/formula/$CLI_IN_VOLUME" --model-sha256 "$actual_model_sha"
  --provenance "git_head=$git_head" --provenance "git_dirty_paths=$git_dirty"
  --provenance "harness_file_git_status=${harness_git_state:-clean}"
  --provenance "cli_source_commit=$cli_source_commit" --provenance "server_image=$reg_image"
  --provenance "n_cpu_moe=$reg_n_cpu_moe" --provenance "runner_sha256=$runner_sha"
  --provenance "registry_key=$key" --provenance "registry_sha256=$registry_sha"
  --provenance "model_sha256_source=$hash_source" --provenance "cases_file_sha256=$cases_file_sha")
run_harness() {  # run_harness <stdout> <stderr> <args...>: prints the exit code, records OOM
  local so=$1 se=$2 rc; shift 2
  set +e
  docker run --pull never --name "$HARNESS" --network "$NET" \
    --user 1000:1000 --cap-drop ALL --security-opt no-new-privileges --read-only --tmpfs /tmp \
    --memory 2g --cpus 2 --pids-limit 128 -e PYTHONDONTWRITEBYTECODE=1 \
    --mount type=bind,src="$ROOT/tools",dst=/tools,readonly \
    --mount type=bind,src="$pack",dst=/pack,readonly \
    --mount type=volume,src=mimik-formula-target,dst=/formula,readonly \
    --mount type=bind,src="$out",dst=/out \
    "$HARNESS_IMAGE" "$@" > "$so" 2> "$se" < /dev/null
  rc=$?
  set -e
  echo "harness_oom_killed=$(docker inspect -f '{{.State.OOMKilled}}' "$HARNESS" 2>/dev/null || echo unknown)" >> "$prov"
  docker rm -f "$HARNESS" >/dev/null 2>&1 || true
  echo "$rc"
}
echo "pack=$pack" >> "$prov"
if [ -n "${PREFLIGHT_CASE:-}" ]; then
  pre=(--split dev --cases "$PREFLIGHT_CASE" --seeds 17 --arm "${ARM:-none}-preflight"
    --output /out/preflight-ledger.jsonl)
  echo "preflight_cli=${common[*]} ${pre[*]}" >> "$prov"
  p0=$(date +%s)
  prc=$(run_harness "$out/preflight.stdout" "$out/preflight.stderr" "${common[@]}" "${pre[@]}")
  echo "preflight_exit=$prc preflight_wall_seconds=$(( $(date +%s) - p0 ))" >> "$prov"
  [ "$prc" = 0 ] || fail 6 preflight "1-case preflight ($PREFLIGHT_CASE) failed with exit $prc"
  # A pipeline check, not a score: one finished episode that is not an error.
  if ! grep -q '"event": "run_summary"' "$out/preflight-ledger.jsonl" \
    || grep -q '"outcome": "error"' "$out/preflight-ledger.jsonl"; then
    fail 6 preflight "1-case preflight ($PREFLIGHT_CASE): ledger incomplete or error episode"
  fi
fi
ledger_name=ledger-$(date +%Y%m%dT%H%M%S).jsonl
arm_args=()
[ -z "${ARM:-}" ] || arm_args=(--arm "$ARM")
cli=("${common[@]}" --split "$hsplit" "${case_args[@]}" --seeds "$seeds" "${arm_args[@]}"
  --budget-seconds "$budget" --output "/out/$ledger_name")
{ echo "harness_cli=${cli[*]}"; echo "ledger_file=$ledger_name"; } >> "$prov"
h0=$(date +%s)
rc=$(run_harness "$out/harness.stdout" "$out/harness.stderr" "${cli[@]}")
echo "harness_exit=$rc harness_wall_seconds=$(( $(date +%s) - h0 ))" >> "$prov"
[ "$rc" = 0 ] || exit "$rc"
