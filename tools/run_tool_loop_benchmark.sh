#!/usr/bin/env bash
# Canonical live runner for tools/tool_loop_benchmark.py (harness v3). Prototype tier.
# Models come from the registry tools/models.json (tools/model_registry.py): a new model is a
# registry entry, never a change here. The held-out split runs only the frozen blind case file
# (plan §11 item 8): HELDOUT_CASES (default: the pin's frozen path) must hash to the pin in
# tools/heldout-v3-pin.json, which must equal the harness's code anchor; the file is mounted
# read-only and the harness verifies it again from one read before parsing.
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
#   HELDOUT_CASES   held-out split only: the frozen case file (default: the pin's path)
#   QUIET=1         harness --quiet-progress (no per-case outcome on stderr; plan §11 item 3)
#   PREFLIGHT_CASE  dev case id: 1-case, seed-17 dev run on the same server before the arm
#   BUDGET_SECONDS  harness run budget (default: seconds left before the cutoff - 900)
#   WINDOW_END      absolute cutoff (epoch s; the queue passes the window end)
#   QUEUE_PID       the queue's pid: the runner stops if that process is gone
#   N_CPU_MOE       llama.cpp --n-cpu-moe (required where the registry says so)
#   DEVICE          gpu (default) | cpu: no /dev/dri in the server container, -ngl 0, request
#                   timeout >= 900 s, health wait >= 1800 s, no fan precondition
#   THREADS         llama.cpp --threads (default: registry; 6 for cpu)
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
# work is refused Monday-Friday 09:00-18:00 local time (AGENTS.md, /etc/localtime): the
# schedule is re-checked immediately before the hash, the pre-warm, the server load, the
# preflight and the harness launch, and every 15 s while the server loads and the harness
# runs; the harness gets the budget left at launch and the absolute --deadline-epoch, so no
# episode starts past the cutoff. tools/run_queue.py and tools/run_watchdog.py are further
# independent stops. The model path is resolved (realpath); the resolved file is size-checked,
# hashed in a locked-down container (or matched in HASH_CACHE), pre-warmed into the page cache
# when the registry says so (plan §11 item 27), and bind-mounted read-only. Server and harness
# run detached on an internal Docker network with no published ports, never pull images, use
# --init, run as uid 1000 with all capabilities dropped and read-only root filesystems. Every
# docker call has a timeout. Cleanup stops the harness and the server first, then collects
# logs, removes every owned container (prefix mimik-toolloop-v3-) and the network, and
# verifies that they are gone (cleanup_verified). provenance.txt holds key=value lines, with
# failure=<class> on any runner failure (tools/run_queue.py reads it).
# Exit codes: 2 usage/config, 3 CLI hash, 4 server start/health/VRAM, 5 model file/hash,
# 6 preflight, 9 another run active, 10 schedule window, 11 cleanup unresolved, 12 docker
# failure, 13 queue gone, 14 GPU fan not manual, 130/143 signal; otherwise the harness exit
# code (3 formula infrastructure fault, 4 fail-fast abort).
set -euo pipefail
export TZ=:/etc/localtime
ROOT=$(cd "$(dirname "$0")/.." && pwd)
MAIN=$(dirname "$(timeout --foreground -k 5 30 git -C "$ROOT" rev-parse --path-format=absolute --git-common-dir)")
RUN_BASE=$ROOT/target/tool-loop-benchmark-$(date +%Y%m%d)
# Local image ids, pinned because the tags may move: mimik-ci (hash checks) and
# mimik-toolloop-py:20261015 (FROM mimik-ci plus chmod 0711 /root so uid 1000 reaches python).
CHECK_IMAGE=sha256:9578747a34f5e02cb7561c8e2a8903b75100b96ce1b0587d5fa4b687be0d3a9f
HARNESS_IMAGE=sha256:8477ff915533b4a21a9727d2804acd06bae15b93e95d60627f8e127cd7025a7d
OWNED=mimik-toolloop-v3-
LABEL=mimik.toolloop=v3
NET=mimik-toolloop-v3
SRV=${OWNED}srv
HARNESS=${OWNED}harness
CLI_IN_VOLUME=harness-formula-eval/release/examples/formula_eval
CLI_SHA=${FORMULA_CLI_SHA256:-f0e13322ef921fa89685c48831e9a7658cf789e842514a8a93ff891fa334fc10}
CLI_BUILD_INFO=${CLI_BUILD_INFO:-$MAIN/target/tool-loop-benchmark-20261015/harness-v2/formula-cli-build.txt}
SMOKE_CASES=dev-001,dev-005,dev-008
WINDOW_MARGIN=900   # no episode starts later than this before the cutoff
STOP_MARGIN=360     # a running load or harness is stopped this long before the cutoff
DOCKER_TIMEOUT=${DOCKER_TIMEOUT:-60}  # per short docker call (tests lower it)
POLL_SECONDS=${RUNNER_POLL_SECONDS:-15}  # harness poll interval (tests lower it)
VRAM_PATTERN='out of memory|OutOfDeviceMemory|failed to allocate|unable to allocate|cannot allocate'
[ $# -ge 2 ] || { sed -n '7,10p' "$0" >&2; exit 2; }
key=$1 split=$2 seeds=${3:-${SEEDS:-17,29}}
exec </dev/null
die() { echo "$2" >&2; exit "$1"; }
is_uint() { [[ $1 =~ ^[0-9]+$ ]]; }
dk() { timeout --foreground -k 10 "$DOCKER_TIMEOUT" docker "$@"; }  # every short docker call is bounded
lock=${LOCK_FILE:-$MAIN/target/.tool-loop-benchmark.lock}
mkdir -p "$(dirname "$lock")"
exec 9>"$lock"
flock -n 9 || die 9 "another tool-loop run is active; runs are strictly sequential"
# One Docker endpoint (second review round): no endpoint override, default context only.
for v in DOCKER_HOST DOCKER_CONTEXT DOCKER_CONFIG; do
  [ -z "${!v:-}" ] || die 2 "refusing to run with $v set (one Docker endpoint only)"
done
docker_context=$(dk context show 2>/dev/null) || die 12 "docker context show failed"
[ "$docker_context" = default ] || die 2 "refusing docker context '$docker_context' (default only)"
RUN_ID=$(date +%Y%m%dT%H%M%S)-$$
LABELS=(--label "$LABEL" --label "mimik.toolloop.run=$RUN_ID")
labelled() {  # labelled <name>: the container exists and carries the ownership label
  [ "$(dk inspect -f '{{index .Config.Labels "mimik.toolloop"}}' "$1" 2>/dev/null)" = v3 ]
}
ours() {  # ours <name>: the container exists and carries this run's label
  [ "$(dk inspect -f '{{index .Config.Labels "mimik.toolloop.run"}}' "$1" 2>/dev/null)" = "$RUN_ID" ]
}
check_registry=${CHECK_REGISTRY:-$MAIN/target/tool-loop-check-containers.txt}
mkdir -p "$(dirname "$check_registry")"
owned_names() {  # owned containers: label AND name prefix; fails if docker cannot list
  local names
  names=$(dk ps -a --filter "label=$LABEL" --format '{{.Names}}') || return 1
  grep "^$OWNED" <<< "$names" || true
}
owned_network() {  # prints the network name if the owned (labelled) network exists
  local names
  names=$(dk network ls --filter "label=$LABEL" --format '{{.Name}}') || return 1
  grep -x "$NET" <<< "$names" || true
}

# ---- configuration (registry + pass-through), validated before anything runs
[ -z "${N_CPU_MOE:-}" ] || is_uint "$N_CPU_MOE" || die 2 "N_CPU_MOE must be a non-negative integer"
[ -z "${WINDOW_END:-}" ] || is_uint "$WINDOW_END" || die 2 "WINDOW_END must be an epoch in seconds"
[ -z "${QUEUE_PID:-}" ] || is_uint "$QUEUE_PID" || die 2 "QUEUE_PID must be a pid"
{ is_uint "$DOCKER_TIMEOUT" && [ "$DOCKER_TIMEOUT" -gt 0 ]; } || die 2 "DOCKER_TIMEOUT must be a positive integer"
is_uint "$POLL_SECONDS" || die 2 "RUNNER_POLL_SECONDS must be an integer"
ctx_arg=${4:-${CTX:-}}
[ -z "$ctx_arg" ] || is_uint "$ctx_arg" || die 2 "ctx must be a positive integer"
reg_cmd=(python3 "$ROOT/tools/model_registry.py" shell "$key")
[ -z "$ctx_arg" ] || reg_cmd+=(--ctx "$ctx_arg")
[ -z "${N_CPU_MOE:-}" ] || reg_cmd+=(--n-cpu-moe "$N_CPU_MOE")
# DEVICE=cpu (GPU cooling fault, 2026-10-16): no /dev/dri in the server container, -ngl 0.
device=${DEVICE:-gpu}
case $device in gpu|cpu) ;; *) die 2 "DEVICE must be gpu or cpu" ;; esac
reg_cmd+=(--device "$device")
if [ -n "${THREADS:-}" ]; then
  { is_uint "$THREADS" && [ "$THREADS" -ge 1 ] && [ "$THREADS" -le 64 ]; } \
    || die 2 "THREADS must be 1..64"
  reg_cmd+=(--threads "$THREADS")
fi
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
reg_vram_budget=''
reg_device=''
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
case ${QUIET:-0} in 0|1) ;; *) die 2 "QUIET must be 0 or 1" ;; esac  # never echoed raw (round 3)
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
fail() {  # fail <exit> <class> <message>: the message never adds a provenance line
  local detail=${3//[$'\t\r\n']/ }
  echo "$detail" >&2
  printf 'failure=%s\nfailure_detail=%s\n' "$2" "$detail" >> "$prov"
  exit "$1"
}
{
  echo "date_start=$(date -Is)"
  echo "runner=tools/run_tool_loop_benchmark.sh harness=v3 key=$key split=$split arm=${ARM:-}"
  echo "window_end=${WINDOW_END:-} queue_pid=${QUEUE_PID:-} vram_budget_bytes=$reg_vram_budget"
  echo "run_id=$RUN_ID docker_context=$docker_context device=$reg_device"
  echo "cpu_temperature=unavailable (no CPU sensor on this host: no k10temp in /sys/class/hwmon)"
} > "$prov"
cases_file_sha=
if [ -n "${CASES_FILE:-}" ]; then
  [ -f "$CASES_FILE" ] || fail 2 config "CASES_FILE $CASES_FILE not found"
  cp -- "$CASES_FILE" "$out/cases-file.json"
  cases_file_sha=$(sha256sum "$out/cases-file.json" | cut -d' ' -f1)
  case_args=(--cases-file /out/cases-file.json)
fi

# ---- cleanup: stop heavy work first, then diagnostics, then remove and verify
vram() { cat /sys/class/drm/card0/device/mem_info_vram_used 2>/dev/null || echo n/a; }
swap_used() { free -m | awk '/^Swap:/{print $3}'; }
cgroup_file() {  # cgroup_file <name>: a file of the server's cgroup (systemd or cgroupfs layout)
  local cid
  cid=$(dk inspect -f '{{.Id}}' "$SRV" 2>/dev/null) || return 1
  cat "/sys/fs/cgroup/system.slice/docker-$cid.scope/$1" 2>/dev/null \
    || cat "/sys/fs/cgroup/docker/$cid/$1" 2>/dev/null
}
started_heavy=0
cleanup() {
  local rc=$? left net listing
  if [ "$started_heavy" = 1 ]; then
    # Heavy work stops before any diagnostics: harness stopped, server frozen (cgroup
    # freezer: no compute, cgroup and VRAM still readable), diagnostics read, server stopped.
    # Fixed names are acted on only when they carry this run's label (residual risk T2).
    if ours "$HARNESS"; then dk stop -t 10 "$HARNESS" >/dev/null 2>&1 || true; fi
    paused=false
    if ours "$SRV" && dk pause "$SRV" >/dev/null 2>&1; then paused=true; fi
    # If the pause failed, the harness has stopped, so the GPU is idle: read them anyway.
    {
      echo "server_paused_for_diagnostics=$paused vram_used_end=$(vram) swap_used_mib_after=$(swap_used)"
      echo "server_memory_peak=$(cgroup_file memory.peak || echo n/a) server_swap_peak=$(cgroup_file memory.swap.peak || echo n/a)"
    } >> "$prov"
    if [ "$paused" = true ]; then dk unpause "$SRV" >/dev/null 2>&1 || true; fi
    if ours "$SRV"; then dk stop -t 10 "$SRV" >/dev/null 2>&1 || true; fi
    echo "server_oom_killed=$(dk inspect -f '{{.State.OOMKilled}}' "$SRV" 2>/dev/null || echo unknown)" >> "$prov"
    timeout --foreground -k 10 120 docker logs "$SRV" > "$out/server.log" 2>&1 || true
    temps --print-peaks >> "$prov"
  fi
  if listing=$(owned_names); then
    for name in $listing; do dk rm -f "$name" >/dev/null 2>&1 || true; done
  else
    for name in "$HARNESS" "$SRV"; do  # cannot list: known names, label confirmed first
      if [ "$(dk inspect -f "{{index .Config.Labels \"mimik.toolloop\"}}" "$name" 2>/dev/null)" = v3 ]; then
        dk rm -f "$name" >/dev/null 2>&1 || true
      fi
    done
  fi
  if [ -n "$(owned_network 2>/dev/null)" ]; then dk network rm "$NET" >/dev/null 2>&1 || true; fi
  # Verified only when both listings succeed and show nothing owned.
  if left=$(owned_names) && net=$(owned_network); then
    if [ -z "$left$net" ]; then
      echo "cleanup_verified=true date_end=$(date -Is)" >> "$prov"
      return "$rc"
    fi
  else
    left="docker listing failed"
  fi
  { echo "cleanup_verified=false cleanup_remaining=${left//$'\n'/,}${net:+,network}"
    echo "failure=cleanup"; echo "date_end=$(date -Is)"; } >> "$prov"
  exit 11
}
trap cleanup EXIT
trap 'echo "stopped_by_signal=TERM" >> "$prov"; exit 143' TERM
trap 'echo "stopped_by_signal=INT" >> "$prov"; exit 130' INT

# ---- schedule gate (AGENTS.md): re-run immediately before every heavy step
override=0
[ "${SCHEDULE_OVERRIDE:-}" = user-approved ] && override=1
[ "$override" = 0 ] || echo "schedule_override=user-approved" >> "$prov"
gate_now='' gate_left='' gate_cutoff=''
window() {  # window <margin>: sets gate_*; returns 1 if the window is closed within margin
  local line cutoff_args=()
  [ -z "${WINDOW_END:-}" ] || cutoff_args=(--cutoff "$WINDOW_END")
  line=$(python3 "$ROOT/tools/run_queue.py" window-check --margin "$1" ${cutoff_args[@]+"${cutoff_args[@]}"}) \
    || { gate_line=$line; return 1; }
  gate_line=$line
  gate_now=$(sed -n 's/.* now=\([0-9]*\).*/\1/p' <<< "$line")
  gate_left=$(sed -n 's/.* seconds_left=\([0-9]*\).*/\1/p' <<< "$line")
  gate_cutoff=$(sed -n 's/.* cutoff=\([0-9]*\).*/\1/p' <<< "$line")
}
gate() {  # gate <step>: refuse the step (exit 10) unless the window stays open past the margin
  if [ "${CHECK_ONLY:-0}" = 1 ]; then  # check-only: never refused; bounded if a window is open
    if [ -z "${WINDOW_END:-}" ] || [ "$override" = 1 ] || ! window "$WINDOW_MARGIN"; then
      gate_now='' gate_cutoff=''
    fi
    return 0
  fi
  if [ "$override" = 1 ]; then gate_now=$(date +%s); gate_left='' gate_cutoff=''; return 0; fi
  window "$WINDOW_MARGIN" || fail 10 window "before $1: ${gate_line:-window-check failed}"
  echo "window_gate_$1=open now=$gate_now seconds_left=$gate_left cutoff=$gate_cutoff" >> "$prov"
}
still_allowed() {  # during a load or a run: stop at the edge, or when the queue is gone
  if [ -n "${QUEUE_PID:-}" ] && ! kill -0 "$QUEUE_PID" 2>/dev/null; then
    fail 13 orphan "queue process $QUEUE_PID is gone during $1"
  fi
  [ "$override" = 1 ] || window "$STOP_MARGIN" || fail 10 window "stopped at the window edge during $1"
}

# ---- model file: resolve symlinks, size, hash (or session cache)
model_real=$(realpath -e -- "$model" 2>/dev/null) || fail 5 model_missing "model file $model not found"
[ -f "$model_real" ] || fail 5 model_missing "model $model_real is not a regular file"
case $model_real in *$'\t'*|*$'\n'*) fail 5 model_missing "model path contains a tab or newline" ;; esac
size=$(stat -c %s "$model_real")
if [ -z "${MODEL_PATH:-}" ] && [ "$size" != "$reg_size" ]; then
  fail 5 model_size "model size $size != registry $reg_size ($model_real)"
fi
check_n=0
window_limit() {  # window_limit <default>: step_limit = min(default, cutoff - now - margin)
  local left
  step_limit=$1 limit_from_window=0
  if [ -n "$gate_cutoff" ] && [ -n "$gate_now" ]; then
    left=$(( gate_cutoff - gate_now - WINDOW_MARGIN ))
    if [ "$left" -lt "$step_limit" ]; then step_limit=$left limit_from_window=1; fi
  fi
}
limit_from_window=0 step_limit=0
check_run() {  # check_run <seconds> <mounts and command...>: a named, bounded check container
  local limit=$1 name rc output; shift
  check_n=$((check_n + 1)); name=${OWNED}check-$$-$check_n
  echo "$name" >> "$check_registry"  # for the watchdog's fallback if docker cannot list
  set +e
  output=$(timeout --foreground -k 10 "$limit" docker run --rm --init --pull never --name "$name" "${LABELS[@]}" --network none \
    --user 1000:1000 --cap-drop ALL --security-opt no-new-privileges --read-only --memory 256m \
    --pids-limit 32 "$@")
  rc=$?
  set -e
  grep -vx "$name" "$check_registry" > "$check_registry.tmp" 2>/dev/null || true
  mv -f "$check_registry.tmp" "$check_registry" 2>/dev/null || true
  if [ "$rc" != 0 ]; then
    dk rm -f "$name" >/dev/null 2>&1 || true
    if [ "$rc" = 124 ] && [ "$limit_from_window" = 1 ]; then
      fail 10 window "check container $name did not finish before the cutoff"
    fi
    fail 12 infra_docker "check container $name failed (exit $rc)"
  fi
  printf '%s\n' "$output"
}
gate hash
limit_from_window=0
cli_line=$(check_run 120 --mount type=volume,src=mimik-formula-target,dst=/formula,readonly \
  "$CHECK_IMAGE" sha256sum "/formula/$CLI_IN_VOLUME")
actual_cli_sha=${cli_line%% *}
if [ "$actual_cli_sha" != "$CLI_SHA" ]; then
  fail 3 cli_hash "formula_eval sha256 $actual_cli_sha != pinned $CLI_SHA"
fi
# ---- held-out hash gate (plan §11 item 8): only the pinned case file, mounted read-only
heldout_mount=() heldout_args=()
if [ "$hsplit" = heldout ]; then
  # The pin is loaded through the harness's own loader, so the code anchor
  # (HELDOUT_FROZEN_SHA256) is checked here too; one value per call keeps paths space-safe.
  pin_value() {
    PYTHONDONTWRITEBYTECODE=1 python3 -c 'import sys; sys.path.insert(0, sys.argv[1]); import tool_loop_benchmark as tl; print(tl.load_heldout_pin(tl.HELDOUT_PIN)[sys.argv[2]])' "$ROOT/tools" "$1"
  }
  heldout_pin_sha=$(pin_value sha256) || fail 2 config "held-out pin unreadable or not the code anchor"
  heldout_pin_file_sha=$(pin_value pin_sha256) || fail 2 config "held-out pin unreadable"
  heldout_default=$(pin_value default_path) || fail 2 config "held-out pin unreadable"
  heldout=${HELDOUT_CASES:-$heldout_default}
  # No whitespace at all: provenance is space-delimited key=value (round 2, Sol 3).
  case $heldout in *[[:space:]]*) fail 2 config "HELDOUT_CASES contains whitespace" ;; esac
  heldout_real=$(realpath -e -- "$heldout" 2>/dev/null) || fail 5 heldout_missing "held-out case file not found"
  case $heldout_real in *[[:space:]]*) fail 2 config "the held-out path contains whitespace" ;; esac
  [ -f "$heldout_real" ] || fail 5 heldout_missing "held-out case file is not a regular file"
  heldout_line=$(check_run 120 --mount type=bind,src="$heldout_real",dst=/heldout/cases-heldout.json,readonly \
    "$CHECK_IMAGE" sha256sum /heldout/cases-heldout.json)
  [ "${heldout_line%% *}" = "$heldout_pin_sha" ] || fail 5 heldout_hash "held-out case file sha256 differs from the pin"
  printf 'heldout_cases_sha256=%s heldout_pin_sha256=%s heldout_cases=%s\n' \
    "${heldout_line%% *}" "$heldout_pin_file_sha" "$heldout_real" >> "$prov"
  heldout_mount=(--mount "type=bind,src=$heldout_real,dst=/heldout/cases-heldout.json,readonly")
  heldout_args=(--heldout-cases /heldout/cases-heldout.json)
fi
IFS=' ' read -r st_size st_mtime st_inode st_dev <<< "$(stat -c '%s %.9Y %i %d' "$model_real")"
hash_source=hashed actual_model_sha=''
if [ -n "${HASH_CACHE:-}" ] && [ -f "$HASH_CACHE" ]; then  # path size mtime_ns inode dev sha (tabs)
  actual_model_sha=$(awk -F'\t' -v p="$model_real" -v s="$st_size" -v m="$st_mtime" \
    -v i="$st_inode" -v d="$st_dev" \
    'NF == 6 && $1 "" == p "" && $2 "" == s "" && $3 "" == m "" && $4 "" == i "" && $5 "" == d "" && length($6) == 64 && $6 ~ /^[0-9a-f]+$/ {h = $6} END {print h}' \
    "$HASH_CACHE")
  [ -z "$actual_model_sha" ] || hash_source=session_cache
fi
h0=$(date +%s)
if [ -z "$actual_model_sha" ]; then
  window_limit $(( size / 5000000 + 600 ))  # >= 5 MB/s; the HDD reads about 12.6 MB/s cold
  [ "$limit_from_window" = 0 ] || echo "hash_limit_seconds=$step_limit (bounded by the window)" >> "$prov"
  model_line=$(check_run "$step_limit" \
    --mount type=bind,src="$model_real",dst=/models/model.gguf,readonly \
    "$CHECK_IMAGE" sha256sum /models/model.gguf)
  actual_model_sha=${model_line%% *}
fi
echo "model_hash_seconds=$(( $(date +%s) - h0 )) model_sha256_source=$hash_source" >> "$prov"
if [ "$actual_model_sha" != "$sha" ]; then
  fail 5 model_hash "model sha256 $actual_model_sha != expected $sha"
fi
if [ -n "${HASH_CACHE:-}" ] && [ "$hash_source" = hashed ]; then
  printf '%s\t%s\t%s\t%s\t%s\t%s\n' "$model_real" "$st_size" "$st_mtime" "$st_inode" "$st_dev" \
    "$actual_model_sha" >> "$HASH_CACHE"
fi
harness_sha=$(sha256sum "$ROOT/tools/tool_loop_benchmark.py" | cut -d' ' -f1)
runner_sha=$(sha256sum "$0" | cut -d' ' -f1)
registry_sha=$(sha256sum "$ROOT/tools/models.json" | cut -d' ' -f1)
git_head=$(timeout --foreground -k 5 30 git -C "$ROOT" rev-parse HEAD)
git_dirty=$(timeout --foreground -k 5 30 git -C "$ROOT" status --porcelain | wc -l)
harness_git_state=$(timeout --foreground -k 5 30 git -C "$ROOT" status --porcelain -- tools/tool_loop_benchmark.py | cut -c1-2)
cli_source_commit=unknown
if [ -f "$CLI_BUILD_INFO" ]; then
  cli_source_commit=$(sed -n 's/^source_commit=\([0-9a-f]*\).*/\1/p' "$CLI_BUILD_INFO")
fi
{
  echo "image=$reg_image"
  dk image inspect "$reg_image" --format 'image_id={{.Id}} version={{index .Config.Labels "org.opencontainers.image.version"}} revision={{index .Config.Labels "org.opencontainers.image.revision"}}' 2>/dev/null \
    || echo "image_id=not-present-locally"
  echo "model=$model resolved=$model_real sha256=$actual_model_sha (expected $sha) size=$size"
  echo "formula_cli=mimik-formula-target:/$CLI_IN_VOLUME sha256=$actual_cli_sha (pinned $CLI_SHA) source_commit=$cli_source_commit ($CLI_BUILD_INFO)"
  echo "harness_sha256=$harness_sha runner_sha256=$runner_sha registry_sha256=$registry_sha"
  echo "git_head=$git_head git_dirty_paths=$git_dirty harness_file_git_status='${harness_git_state:-clean}'"
  echo "check_image=$CHECK_IMAGE harness_image=$HARNESS_IMAGE server_memory=$srv_memory"
  echo "ctx=$ctx thinking=$thinking thinking_capability=$reg_thinking tools=$reg_tools"
  echo "cases_file_sha256=$cases_file_sha"
  echo "server_args=${reg_server_args[*]}"
  echo "prompt_cache=on (server: no --no-cache-prompt; harness: --cache-prompt; --cache-ram 0 = no extra host-RAM cache)"
} >> "$prov"
if [ "${CHECK_ONLY:-0}" = 1 ]; then
  echo "check_only=1 (stopped before launch)" >> "$prov"; exit 0
fi
if [ "$reg_prewarm" = 1 ]; then  # a cold read of a big GGUF on the HDD outlasts the health wait
  gate prewarm
  p0=$(date +%s)
  window_limit $(( size / 5000000 + 600 ))
  set +e
  timeout --foreground -k 10 "$step_limit" cat -- "$model_real" > /dev/null
  prc=$?
  set -e
  if [ "$prc" = 124 ] && [ "$limit_from_window" = 1 ]; then
    fail 10 window "pre-warm did not finish before the cutoff"
  fi
  [ "$prc" = 0 ] || fail 5 model_missing "pre-warm read of $model_real failed or timed out"
  echo "prewarm_seconds=$(( $(date +%s) - p0 ))" >> "$prov"
fi

# ---- server
gate server
echo "vram_used_before=$(vram) swap_used_mib_before=$(swap_used)" >> "$prov"
temps() {  # temperatures from /sys/class/hwmon by sensor name; the peak file keeps maxima
  timeout --foreground -k 5 30 python3 "$ROOT/tools/toolloop_ops.py" temps \
    --peak-file "$out/temps-peak.json" "$@" 2>/dev/null || echo "temp_sensors=unreadable"
}
temps --prefix before >> "$prov"
# The GPU fan must be manual at >= 230 (auto held ~22 % PWM while the junction reached SW CTF
# on 2026-10-16); the manual setting resets on reboot.
gpu_devices=()
if [ "$reg_device" = gpu ]; then
  if ! fan_line=$(timeout --foreground -k 5 30 python3 "$ROOT/tools/toolloop_ops.py" fancheck \
    2> "$out/fancheck.err"); then
    fail 14 fan_not_manual "$(tr '\n' ' ' < "$out/fancheck.err")"
  fi
  echo "$fan_line" >> "$prov"
  dri=/dev/dri  # the test tree replaces it only in test mode
  if [ "${MIMIK_TEST_MODE:-}" = 1 ] && [ -n "${MIMIK_TEST_DRI:-}" ]; then dri=$MIMIK_TEST_DRI; fi
  if ! render_gid=$(stat -c %g "$dri/renderD128" 2>/dev/null) \
    || ! card_gid=$(stat -c %g "$dri/card0" 2>/dev/null); then
    fail 12 infra_docker "GPU device nodes $dri/renderD128 or $dri/card0 missing"
  fi
  gpu_devices=(--device "$dri/renderD128" --device "$dri/card0"
    --group-add "$render_gid" --group-add "$card_gid")
else
  echo "fan_precondition=skipped (device cpu: no GPU device in the container)" >> "$prov"
fi
if net_state=$(dk network inspect -f '{{.Internal}} {{index .Labels "mimik.toolloop"}}' "$NET" 2>/dev/null); then
  [ "$net_state" = "true v3" ] || fail 12 infra_docker "network $NET exists and is not internal and owned ($net_state)"
else
  dk network create --internal "${LABELS[@]}" "$NET" >/dev/null || fail 12 infra_docker "network create failed"
fi
started_heavy=1
t0=$(date +%s)
dk run -d --init --pull never --name "$SRV" "${LABELS[@]}" --network "$NET" --network-alias server \
  --user 1000:1000 --cap-drop ALL --security-opt no-new-privileges --read-only --tmpfs /tmp \
  --memory "$srv_memory" --memory-swap "$srv_memory" --cpus 6 --pids-limit 512 \
  ${gpu_devices[@]+"${gpu_devices[@]}"} \
  --mount type=bind,src="$model_real",dst=/models/model.gguf,readonly \
  "$reg_image" ${reg_server_args[@]+"${reg_server_args[@]}"} >/dev/null || fail 4 server_start "server container did not start"
server_failed() {  # server_failed <class> <message>: a VRAM allocation failure is infra_vram
  if timeout --foreground -k 10 60 docker logs "$SRV" 2>&1 | grep -Eqi "$VRAM_PATTERN"; then
    fail 4 infra_vram "$2 (VRAM allocation failure in the server log)"
  fi
  fail 4 "$1" "$2"
}
healthy=0
while [ $(( $(date +%s) - t0 )) -lt "$health_wait" ]; do
  if timeout --foreground -k 5 10 docker exec "$SRV" curl -sf http://127.0.0.1:8080/health >/dev/null 2>&1; then healthy=1; break; fi
  state=$(dk inspect -f '{{.State.Running}}' "$SRV" 2>/dev/null || echo gone)
  [ "$state" = true ] || server_failed server_start "server exited during load"
  still_allowed "server load"
  temps >/dev/null
  sleep 2
done
[ $healthy = 1 ] || server_failed server_health "server not healthy after ${health_wait}s"
echo "server_ready_seconds=$(( $(date +%s) - t0 )) vram_used_loaded=$(vram)" >> "$prov"
timeout --foreground -k 5 30 docker exec "$SRV" curl -sf http://127.0.0.1:8080/props > "$out/props.json" || true

# ---- harness (detached; polled for the window edge and the queue)
quiet=()
[ "${QUIET:-0}" = 1 ] && quiet=(--quiet-progress)
if [ "$hsplit" = heldout ]; then  # held-out progress is never per case (round 2, Sonnet M1)
  [ "${QUIET:-1}" = 1 ] || echo "quiet_forced=heldout (QUIET=0 ignored)" >> "$prov"
  quiet=(--quiet-progress)
fi
common=(python3 /tools/tool_loop_benchmark.py --pack /pack --endpoint http://server:8080
  --model-label "$reg_label" --temperature "$temperature" --top-p "$top_p"
  --max-tokens "$max_tokens" --max-turns "$max_turns" --thinking "$thinking"
  --thinking-capability "$reg_thinking" --cache-prompt --tool-choice auto --device "$reg_device"
  --request-timeout "$request_timeout" --ctx "$ctx" "${flags[@]}" ${quiet[@]+"${quiet[@]}"}
  --server-config "$reg_server_config"
  --formula-cli "/formula/$CLI_IN_VOLUME" --model-sha256 "$actual_model_sha"
  --provenance "git_head=$git_head" --provenance "git_dirty_paths=$git_dirty"
  --provenance "harness_file_git_status=${harness_git_state:-clean}"
  --provenance "cli_source_commit=$cli_source_commit" --provenance "server_image=$reg_image"
  --provenance "n_cpu_moe=$reg_n_cpu_moe" --provenance "runner_sha256=$runner_sha"
  --provenance "registry_key=$key" --provenance "registry_sha256=$registry_sha"
  --provenance "model_sha256_source=$hash_source" --provenance "cases_file_sha256=$cases_file_sha")
timing() {  # timing <phase>: gate, then budget and absolute deadline from this moment
  gate "$1"
  phase_budget=${BUDGET_SECONDS:-0} phase_deadline=()
  if [ -n "$gate_cutoff" ]; then
    local left=$(( gate_cutoff - WINDOW_MARGIN - gate_now ))
    if [ "$phase_budget" = 0 ] || [ "$phase_budget" -gt "$left" ]; then phase_budget=$left; fi
    phase_deadline=(--deadline-epoch "$(( gate_cutoff - WINDOW_MARGIN ))")
  fi
  echo "${1}_start_epoch=$gate_now ${1}_budget_seconds=$phase_budget ${1}_cutoff=$gate_cutoff" >> "$prov"
}
run_harness() {  # run_harness <phase> <args...>: sets phase_rc; records <phase>_exit/_oom_killed
  local phase=$1 status limit started; shift
  if labelled "$HARNESS"; then dk rm -f "$HARNESS" >/dev/null 2>&1 || true; fi  # a leftover
  dk run -d --init --pull never --name "$HARNESS" "${LABELS[@]}" --network "$NET" \
    --user 1000:1000 --cap-drop ALL --security-opt no-new-privileges --read-only --tmpfs /tmp \
    --memory 2g --cpus 2 --pids-limit 128 -e PYTHONDONTWRITEBYTECODE=1 \
    --mount type=bind,src="$ROOT/tools",dst=/tools,readonly \
    --mount type=bind,src="$pack",dst=/pack,readonly \
    --mount type=volume,src=mimik-formula-target,dst=/formula,readonly \
    --mount type=bind,src="$out",dst=/out ${heldout_mount[@]+"${heldout_mount[@]}"} \
    "$HARNESS_IMAGE" "$@" >/dev/null || fail 12 infra_docker "harness container did not start ($phase)"
  started=$(date +%s)
  limit=$(( phase_budget > 0 ? phase_budget + max_turns * ${request_timeout%.*} + 600 : 0 ))
  while :; do
    status=$(dk inspect -f '{{.State.Status}} {{.State.ExitCode}} {{.State.OOMKilled}}' "$HARNESS") \
      || fail 12 infra_docker "docker inspect of the harness failed ($phase)"
    case $status in exited*|dead*) break ;; esac
    still_allowed "$phase"
    temps >/dev/null
    if [ "$limit" -gt 0 ] && [ $(( $(date +%s) - started )) -gt "$limit" ]; then
      fail 12 infra_docker "harness exceeded its time limit ${limit}s ($phase)"
    fi
    sleep "$POLL_SECONDS"
  done
  read -r _ phase_rc oom <<< "$status"
  timeout --foreground -k 10 300 docker logs "$HARNESS" > "$out/$phase.stdout" 2> "$out/$phase.stderr" || true
  if ours "$HARNESS"; then dk rm -f "$HARNESS" >/dev/null 2>&1 || true; fi
  echo "${phase}_exit=$phase_rc ${phase}_oom_killed=$oom ${phase}_wall_seconds=$(( $(date +%s) - started ))" >> "$prov"
}
echo "pack=$pack" >> "$prov"
if [ -n "${PREFLIGHT_CASE:-}" ]; then
  timing preflight
  pre=(--split dev --cases "$PREFLIGHT_CASE" --seeds 17 --arm "${ARM:-none}-preflight"
    --budget-seconds "$phase_budget" ${phase_deadline[@]+"${phase_deadline[@]}"} --output /out/preflight-ledger.jsonl)
  echo "preflight_cli=${common[*]} ${pre[*]}" >> "$prov"
  run_harness preflight "${common[@]}" "${pre[@]}"
  [ "$phase_rc" = 0 ] || fail 6 preflight "1-case preflight ($PREFLIGHT_CASE) failed with exit $phase_rc"
  # A pipeline check, not a score: one finished episode that is neither an error nor not_run.
  if ! grep -q '"event": "run_summary"' "$out/preflight-ledger.jsonl" \
    || grep -Eq '"outcome": "(error|not_run)"' "$out/preflight-ledger.jsonl"; then
    fail 6 preflight "1-case preflight ($PREFLIGHT_CASE): ledger incomplete, error or not_run"
  fi
fi
ledger_name=ledger-$(date +%Y%m%dT%H%M%S).jsonl
arm_args=()
[ -z "${ARM:-}" ] || arm_args=(--arm "$ARM")
timing main
cli=("${common[@]}" --split "$hsplit" ${heldout_args[@]+"${heldout_args[@]}"} ${case_args[@]+"${case_args[@]}"} --seeds "$seeds" ${arm_args[@]+"${arm_args[@]}"}
  --budget-seconds "$phase_budget" ${phase_deadline[@]+"${phase_deadline[@]}"} --output "/out/$ledger_name")
{ echo "harness_cli=${cli[*]}"; echo "ledger_file=$ledger_name"; } >> "$prov"
run_harness main "${cli[@]}"
[ "$phase_rc" = 0 ] || exit "$phase_rc"
