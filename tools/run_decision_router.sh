#!/usr/bin/env bash
# Runner for one decision-router unit (docs/evaluation-tooling.md). Prototype tier.
# Same safety contract as tools/run_tool_loop_benchmark.sh: the shared runner lock (strictly
# sequential with the tool-loop runs), one Docker endpoint, schedule gates before every heavy
# step, an absolute cutoff, ownership labels on every container, bounded docker calls, verified
# cleanup and key=value provenance (failure=<class>) that tools/run_queue.py reads.
#
# Usage: tools/run_decision_router.sh <router-key> <unit>
#   router-key  a key of tools/decision_models.json (python3 tools/decision_models.py list)
#   unit        probe | screen | ablation
# Environment:
#   OUT_DIR         output directory (default target/decision-router-<date>/<key>/<unit>-<time>)
#   WINDOW_END      absolute cutoff (epoch s; the queue passes the window or session end)
#   QUEUE_PID       the queue's pid: the runner stops if that process is gone
#   PROBE_SUMMARY   screen/ablation: the router's complete probe summary.json (else refused)
#   HASH_CACHE      file of artefact hashes verified this session (the queue sets it)
#   CHECK_ONLY=1    stop after the pre-launch checks, hashes, states and provenance
#   SCHEDULE_OVERRIDE=user-approved   heavy work inside the weekday 09:00-18:00 block
#   MIMIK_MODEL_ROOT  model root (default: the registry's)
# The deadline is the earlier of WINDOW_END and HARD_DEADLINE (Monday 2026-10-19 09:00, plan
# §5). The driver gets deadline - 900 s and starts no decision whose timeout would cross that
# minus 120 s; the runner stops the container 360 s before the deadline (window margin). The router container mimik-toolloop-v3-router runs with --network none, --pull never,
# --init, uid 1000, all capabilities dropped, a read-only root, read-only model and state mounts,
# memory and CPU caps and no /dev/dri (CPU only while the GPU is out of service). Only the
# prepared states and plans are mounted into it; the host-only key -> case id mapping
# (prep/run/host) never is.
# Exit codes: 2 usage/config, 3 CLI hash or ineligible router, 5 artefact missing/hash,
# 9 another run active, 10 schedule window/deadline, 11 cleanup unresolved, 12 docker failure,
# 13 queue gone, 130/143 signal; otherwise the driver's exit code (1 = unit incomplete).
set -euo pipefail
export TZ=:/etc/localtime
ROOT=$(cd "$(dirname "$0")/.." && pwd)
MAIN=$(dirname "$(timeout --foreground -k 5 30 git -C "$ROOT" rev-parse --path-format=absolute --git-common-dir)")
CHECK_IMAGE=sha256:9578747a34f5e02cb7561c8e2a8903b75100b96ce1b0587d5fa4b687be0d3a9f
HARNESS_IMAGE=sha256:8477ff915533b4a21a9727d2804acd06bae15b93e95d60627f8e127cd7025a7d
OWNED=mimik-toolloop-v3-
LABEL=mimik.toolloop=v3
ROUTER=${OWNED}router
CLI_IN_VOLUME=harness-formula-eval/release/examples/formula_eval
CLI_SHA=${FORMULA_CLI_SHA256:-f0e13322ef921fa89685c48831e9a7658cf789e842514a8a93ff891fa334fc10}
HARD_DEADLINE=$(date -d '2026-10-19 09:00' +%s)
if [ "${MIMIK_TEST_MODE:-}" = 1 ] && [ -n "${MIMIK_TEST_HARD_DEADLINE:-}" ]; then
  HARD_DEADLINE=$MIMIK_TEST_HARD_DEADLINE  # tests only
fi
WINDOW_MARGIN=900
STOP_MARGIN=360
DOCKER_TIMEOUT=${DOCKER_TIMEOUT:-60}
POLL_SECONDS=${RUNNER_POLL_SECONDS:-15}
[ $# -eq 2 ] || { sed -n '7,10p' "$0" >&2; exit 2; }
key=$1 unit=$2
exec </dev/null
die() { echo "$2" >&2; exit "$1"; }
is_uint() { [[ $1 =~ ^[0-9]+$ ]]; }
dk() { timeout --foreground -k 10 "$DOCKER_TIMEOUT" docker "$@"; }
case $unit in probe|screen|ablation) ;; *) die 2 "unit must be probe, screen or ablation" ;; esac
lock=${LOCK_FILE:-$MAIN/target/.tool-loop-benchmark.lock}
mkdir -p "$(dirname "$lock")"
exec 9>"$lock"
flock -n 9 || die 9 "another tool-loop or router run is active; runs are strictly sequential"
for v in DOCKER_HOST DOCKER_CONTEXT DOCKER_CONFIG; do
  [ -z "${!v:-}" ] || die 2 "refusing to run with $v set (one Docker endpoint only)"
done
docker_context=$(dk context show 2>/dev/null) || die 12 "docker context show failed"
[ "$docker_context" = default ] || die 2 "refusing docker context '$docker_context' (default only)"
RUN_ID=$(date +%Y%m%dT%H%M%S)-$$
LABELS=(--label "$LABEL" --label "mimik.toolloop.run=$RUN_ID")
ours() {
  [ "$(dk inspect -f '{{index .Config.Labels "mimik.toolloop.run"}}' "$1" 2>/dev/null)" = "$RUN_ID" ]
}
check_registry=${CHECK_REGISTRY:-$MAIN/target/tool-loop-check-containers.txt}
mkdir -p "$(dirname "$check_registry")"
owned_names() {
  local names
  names=$(dk ps -a --filter "label=$LABEL" --format '{{.Names}}') || return 1
  grep "^$OWNED" <<< "$names" || true
}
[ -z "${WINDOW_END:-}" ] || is_uint "$WINDOW_END" || die 2 "WINDOW_END must be an epoch in seconds"
[ -z "${QUEUE_PID:-}" ] || is_uint "$QUEUE_PID" || die 2 "QUEUE_PID must be a pid"
is_uint "$POLL_SECONDS" || die 2 "RUNNER_POLL_SECONDS must be an integer"
dm_router_id='' dm_label='' dm_runtime='' dm_image='' dm_library_revision='' dm_threads=''
dm_n_ctx='' dm_memory='' dm_root='' dm_total_bytes='' dm_hf_cache='' dm_kev_checkpoint=''
dm_gguf='' dm_gguf_sha256='' dm_files=()
reg=$(python3 "$ROOT/tools/decision_models.py" shell "$key") || exit 2
# decision_models.py prints only shlex-quoted assignments (tools/test_decision_models.py).
eval "$reg"
out=${OUT_DIR:-$ROOT/target/decision-router-$(date +%Y%m%d)/$key/$unit-$(date +%Y%m%dT%H%M%S)}
mkdir -p "$out/prep" "$out/unit"
prov=$out/provenance.txt
fail() {  # fail <exit> <class> <message>
  echo "$3" >&2
  { echo "failure=$2"; echo "failure_detail=$3"; } >> "$prov"
  exit "$1"
}
deadline=$HARD_DEADLINE
if [ -n "${WINDOW_END:-}" ] && [ "$WINDOW_END" -lt "$deadline" ]; then deadline=$WINDOW_END; fi
{
  echo "date_start=$(date -Is)"
  echo "runner=tools/run_decision_router.sh key=$key unit=$unit router_id=$dm_router_id runtime=$dm_runtime"
  echo "window_end=${WINDOW_END:-} hard_deadline=$HARD_DEADLINE deadline_epoch=$deadline queue_pid=${QUEUE_PID:-}"
  echo "run_id=$RUN_ID docker_context=$docker_context device=cpu threads=$dm_threads n_ctx=$dm_n_ctx memory=$dm_memory"
  echo "cpu_temperature=unavailable (no CPU sensor on this host: no k10temp in /sys/class/hwmon)"
} > "$prov"

started_heavy=0
cleanup() {
  local rc=$? left
  if [ "$started_heavy" = 1 ]; then
    if ours "$ROUTER"; then dk stop -t 10 "$ROUTER" >/dev/null 2>&1 || true; fi
    echo "router_oom_killed=$(dk inspect -f '{{.State.OOMKilled}}' "$ROUTER" 2>/dev/null || echo unknown)" >> "$prov"
    timeout --foreground -k 10 120 docker logs "$ROUTER" > "$out/router.log" 2>&1 || true
    temps --print-peaks >> "$prov"
  fi
  if listing=$(owned_names); then
    for name in $listing; do
      if [ "$name" = "$ROUTER" ] || [[ $name == "${OWNED}check-$$-"* ]]; then
        dk rm -f "$name" >/dev/null 2>&1 || true
      fi
    done
  fi
  # Verified only when the listing succeeds and shows none of this runner's containers.
  if left=$(owned_names); then
    left=$(grep -E "^(${ROUTER}|${OWNED}check-$$-[0-9]+)\$" <<< "$left" || true)
    if [ -z "$left" ]; then
      echo "cleanup_verified=true date_end=$(date -Is)" >> "$prov"
      return "$rc"
    fi
  else
    left="docker listing failed"
  fi
  { echo "cleanup_verified=false cleanup_remaining=${left//$'\n'/,}"
    echo "failure=cleanup"; echo "date_end=$(date -Is)"; } >> "$prov"
  exit 11
}
trap cleanup EXIT
trap 'echo "stopped_by_signal=TERM" >> "$prov"; exit 143' TERM
trap 'echo "stopped_by_signal=INT" >> "$prov"; exit 130' INT

override=0
[ "${SCHEDULE_OVERRIDE:-}" = user-approved ] && override=1
[ "$override" = 0 ] || echo "schedule_override=user-approved" >> "$prov"
gate_line=''
window() {  # window <margin>: 1 if the window is closed within margin of the cutoff
  gate_line=$(python3 "$ROOT/tools/run_queue.py" window-check --margin "$1" --cutoff "$deadline") || return 1
}
gate() {  # gate <step>
  if [ "${CHECK_ONLY:-0}" = 1 ] || [ "$override" = 1 ]; then return 0; fi
  window "$WINDOW_MARGIN" || fail 10 window "before $1: ${gate_line:-window-check failed}"
  echo "window_gate_$1=open $gate_line" >> "$prov"
}
now() {  # epoch seconds; the test clock only in test mode
  if [ "${MIMIK_TEST_MODE:-}" = 1 ] && [ -n "${MIMIK_TEST_CLOCK_FILE:-}" ]; then
    cut -d. -f1 < "$MIMIK_TEST_CLOCK_FILE"
  else
    date +%s
  fi
}
still_allowed() {
  if [ -n "${QUEUE_PID:-}" ] && ! kill -0 "$QUEUE_PID" 2>/dev/null; then
    fail 13 orphan "queue process $QUEUE_PID is gone during $1"
  fi
  [ "$override" = 1 ] || window "$STOP_MARGIN" || fail 10 window "stopped at the window edge during $1"
  [ "$(now)" -lt "$deadline" ] || fail 10 window "deadline reached during $1"
}
setup_step() {  # setup_step <step>: before every setup step, also with CHECK_ONLY (review Sol 3)
  if [ -n "${QUEUE_PID:-}" ] && ! kill -0 "$QUEUE_PID" 2>/dev/null; then
    fail 13 orphan "queue process $QUEUE_PID is gone before $1"
  fi
  [ $(( deadline - $(now) - STOP_MARGIN )) -gt 0 ] || fail 10 window "before $1: the absolute cutoff has passed"
  gate "$1"
}
step_limit=0 step_bounded=0
bounded() {  # bounded <default seconds> <step>: step_limit = min(default, cutoff - now - margin)
  local left
  left=$(( deadline - $(now) - STOP_MARGIN ))
  [ "$left" -gt 0 ] || fail 10 window "before $2: the absolute cutoff has passed"
  step_limit=$1 step_bounded=0
  if [ "$left" -lt "$step_limit" ]; then
    step_limit=$left step_bounded=1
    echo "${2// /_}_limit_seconds=$left (bounded by the cutoff)" >> "$prov"
  fi
}
temps() {
  timeout --foreground -k 5 30 python3 "$ROOT/tools/toolloop_ops.py" temps \
    --peak-file "$out/temps-peak.json" "$@" 2>/dev/null || echo "temp_sensors=unreadable"
}
check_n=0
check_run() {  # check_run <seconds> <docker run args...>: a named, bounded check container
  local limit=$1 name rc output; shift
  check_n=$((check_n + 1)); name=${OWNED}check-$$-$check_n
  echo "$name" >> "$check_registry"
  set +e
  output=$(timeout --foreground -k 10 "$limit" docker run --rm --init --pull never --name "$name" "${LABELS[@]}" --network none \
    --user 1000:1000 --cap-drop ALL --security-opt no-new-privileges --read-only --tmpfs /tmp \
    --pids-limit 64 "$@")
  rc=$?
  set -e
  grep -vx "$name" "$check_registry" > "$check_registry.tmp" 2>/dev/null || true
  mv -f "$check_registry.tmp" "$check_registry" 2>/dev/null || true
  if [ "$rc" != 0 ]; then
    dk rm -f "$name" >/dev/null 2>&1 || true
    if [ "$rc" = 124 ] && [ "$step_bounded" = 1 ]; then
      fail 10 window "check container $name did not finish before the cutoff"
    fi
    fail 12 infra_docker "check container $name failed (exit $rc)"
  fi
  printf '%s\n' "$output"
}

# ---- registered runs refuse a dirty tree (review m8): the frozen copy must be what ran
git_dirty=$(timeout --foreground -k 5 30 git -C "$ROOT" status --porcelain | wc -l)
if [ -n "${QUEUE_PID:-}" ] && [ "$git_dirty" != 0 ]; then
  fail 2 dirty_tree "a queued run needs a clean tree ($git_dirty dirty paths)"
fi
root_real=$(realpath -e -- "$dm_root" 2>/dev/null) || fail 5 model_missing "model root $dm_root not found"
inside_root() { [[ $1 == "$root_real"/* ]]; }

# ---- formula CLI (the state builder renders through the harness's C0 path)
setup_step hash
bounded 120 "cli hash"
cli_line=$(check_run "$step_limit" --memory 256m --mount type=volume,src=mimik-formula-target,dst=/formula,readonly \
  "$CHECK_IMAGE" sha256sum "/formula/$CLI_IN_VOLUME")
[ "${cli_line%% *}" = "$CLI_SHA" ] || fail 3 cli_hash "formula_eval sha256 ${cli_line%% *} != pinned $CLI_SHA"

# ---- artefacts: resolve, size, hash (or session cache); disk rate from what was hashed
hashed_bytes=0 hash_seconds=0
declare -a mounts=()
for spec in "${dm_files[@]}"; do
  IFS=$'\t' read -r role rel size sha <<< "$spec"
  setup_step "hash_$role"
  path=$(realpath -e -- "$dm_root/$rel" 2>/dev/null) || fail 5 model_missing "artefact $role ($dm_root/$rel) not found"
  inside_root "$path" || fail 5 model_path "artefact $role resolves outside the model root ($path)"
  [ -f "$path" ] || fail 5 model_missing "artefact $role is not a regular file"
  [ "$(stat -c %s "$path")" = "$size" ] || fail 5 model_size "artefact $role size differs from the registry"
  IFS=' ' read -r st_size st_mtime st_inode st_dev <<< "$(stat -c '%s %.9Y %i %d' "$path")"
  actual=''
  if [ -n "${HASH_CACHE:-}" ] && [ -f "$HASH_CACHE" ]; then
    actual=$(awk -F'\t' -v p="$path" -v s="$st_size" -v m="$st_mtime" -v i="$st_inode" -v d="$st_dev" \
      'NF == 6 && $1 "" == p "" && $2 "" == s "" && $3 "" == m "" && $4 "" == i "" && $5 "" == d "" && length($6) == 64 {h = $6} END {print h}' "$HASH_CACHE")
  fi
  source=session_cache
  if [ -z "$actual" ]; then
    source=hashed
    h0=$(now)
    bounded $(( size / 5000000 + 600 )) "hash $role"
    line=$(check_run "$step_limit" --memory 256m \
      --mount type=bind,src="$path",dst=/artefact,readonly "$CHECK_IMAGE" sha256sum /artefact)
    actual=${line%% *}
    hashed_bytes=$(( hashed_bytes + size )) hash_seconds=$(( hash_seconds + $(now) - h0 ))
    if [ -n "${HASH_CACHE:-}" ]; then
      printf '%s\t%s\t%s\t%s\t%s\t%s\n' "$path" "$st_size" "$st_mtime" "$st_inode" "$st_dev" "$actual" >> "$HASH_CACHE"
    fi
  fi
  [ "$actual" = "$sha" ] || fail 5 model_hash "artefact $role sha256 $actual != registry $sha"
  echo "artefact role=$role sha256=$actual size=$size source=$source" >> "$prov"
done
disk_rate=null
if [ "$hashed_bytes" -gt 100000000 ]; then
  disk_rate=$(( hashed_bytes / (hash_seconds > 0 ? hash_seconds : 1) ))
fi
if [ "$dm_runtime" = kev-torch ]; then
  cache=$(realpath -e -- "$dm_root/$dm_hf_cache") || fail 5 model_missing "Kev hf cache missing"
  inside_root "$cache" || fail 5 model_path "Kev hf cache resolves outside the model root ($cache)"
  mounts=(--mount "type=bind,src=$cache,dst=/hf,readonly")
  model_args=(--backend kev --kev-checkpoint "/hf/${dm_kev_checkpoint#"$dm_hf_cache"/}")
  env_args=(-e HF_HUB_CACHE=/hf -e HF_HUB_OFFLINE=1 -e TRANSFORMERS_OFFLINE=1 -e HF_HUB_DISABLE_TELEMETRY=1)
else
  gguf=$(realpath -e -- "$dm_root/$dm_gguf")
  inside_root "$gguf" || fail 5 model_path "GGUF resolves outside the model root ($gguf)"
  mounts=(--mount "type=bind,src=$gguf,dst=/models/model.gguf,readonly")
  model_args=(--backend anyjev --model /models/model.gguf --model-sha256 "$dm_gguf_sha256" --device cpu)
  env_args=()
fi
echo "hashed_bytes=$hashed_bytes hash_seconds=$hash_seconds disk_rate=$disk_rate" >> "$prov"

# ---- states (isolated builder process, harness image, pinned formula CLI)
setup_step states
bounded 900 "states"
prep_mounts=()
for f in decision_router.py router_state_builder.py tool_loop_benchmark.py; do
  prep_mounts+=(--mount "type=bind,src=$ROOT/tools/$f,dst=/tools/$f,readonly")
done
# The frozen S40 subset is a state-generation input too (round 2, R1): mounted and named.
SUBSET_REL=tools/case-subsets/dev-screen-40.json
prep_mounts+=(--mount "type=bind,src=$ROOT/$SUBSET_REL,dst=/$SUBSET_REL,readonly")
check_run "$step_limit" --memory 2g -e PYTHONDONTWRITEBYTECODE=1 "${prep_mounts[@]}" \
  --mount type=bind,src="$ROOT/examples/tool-loop-demo",dst=/pack,readonly \
  --mount type=volume,src=mimik-formula-target,dst=/formula,readonly \
  --mount type=bind,src="$out/prep",dst=/prep \
  -w /tools "$HARNESS_IMAGE" python3 /tools/decision_router.py prepare --pack /pack \
  --out /prep/run --subset "/$SUBSET_REL" --formula-cli "/formula/$CLI_IN_VOLUME" > "$out/prepare.json"
states_sha=$(sha256sum "$out/prep/run/prepared/states.jsonl" | cut -d' ' -f1)
{
  echo "image=$dm_image library_revision='$dm_library_revision' label=$dm_label"
  echo "states_sha256=$states_sha router_sha256=$(sha256sum "$ROOT/tools/decision_router.py" | cut -d' ' -f1)"
  echo "builder_sha256=$(sha256sum "$ROOT/tools/router_state_builder.py" | cut -d' ' -f1) driver_sha256=$(sha256sum "$ROOT/tools/router_driver.py" | cut -d' ' -f1)"
  echo "worker_sha256=$(sha256sum "$ROOT/tools/local_decision_benchmark.py" | cut -d' ' -f1) registry_sha256=$(sha256sum "$ROOT/tools/decision_models.json" | cut -d' ' -f1)"
  echo "runner_sha256=$(sha256sum "$0" | cut -d' ' -f1) git_head=$(timeout --foreground -k 5 30 git -C "$ROOT" rev-parse HEAD)"
  echo "git_dirty_paths=$(timeout --foreground -k 5 30 git -C "$ROOT" status --porcelain | wc -l)"
} >> "$prov"

# ---- timeouts: the probe's own caps, or a complete probe (else the router is ineligible)
probe_args=()
if [ "$unit" != probe ]; then
  [ -n "${PROBE_SUMMARY:-}" ] && [ -f "$PROBE_SUMMARY" ] || fail 3 ineligible "no probe summary for $key: the router is ineligible"
  probe_args=(--probe-summary "$PROBE_SUMMARY")
fi
tjson=$(python3 "$ROOT/tools/decision_router.py" timeouts --unit "$unit" --router-key "$key" \
  --total-bytes "$dm_total_bytes" "${probe_args[@]}" 2> "$out/timeouts.err") \
  || fail 3 ineligible "$(tr '\n' ' ' < "$out/timeouts.err")"
read -r ready_timeout decision_timeout max_restarts projection < <(python3 -c 'import json,sys; d=json.loads(sys.argv[1]); print(int(d["ready_timeout"]) + 1, int(d["decision_timeout"]), d["max_restarts"], int(d["projection_seconds"]))' "$tjson")
echo "ready_timeout=$ready_timeout decision_timeout=$decision_timeout max_restarts=$max_restarts projection_seconds=$projection" >> "$prov"
rate_args=()
[ "$disk_rate" = null ] || rate_args=(--disk-rate "$disk_rate")
python3 "$ROOT/tools/decision_models.py" summary-extra "$key" "${rate_args[@]}" > "$out/prep/summary-extra.json" \
  || fail 2 config "summary-extra for $key failed"
if [ "${CHECK_ONLY:-0}" = 1 ]; then
  [ $(( deadline - $(now) )) -gt 0 ] || fail 10 window "CHECK_ONLY finished after the absolute cutoff"
  echo "check_only=1 (stopped before launch)" >> "$prov"; exit 0
fi

# ---- router container (detached; polled for the window edge, the deadline and the queue)
setup_step router
# An allowlist of the modules the driver and worker import, never the whole tools/ (review m8).
tool_mounts=()
for f in router_driver.py decision_router.py router_state_builder.py tool_loop_benchmark.py \
  local_decision_benchmark.py local_rag_benchmark.py; do
  tool_mounts+=(--mount "type=bind,src=$ROOT/tools/$f,dst=/tools/$f,readonly")
done
temps --prefix before >> "$prov"
started_heavy=1
t0=$(date +%s)
dk run -d --init --pull never --name "$ROUTER" "${LABELS[@]}" --network none \
  --user 1000:1000 --cap-drop ALL --security-opt no-new-privileges --read-only --tmpfs /tmp \
  --memory "$dm_memory" --memory-swap "$dm_memory" --cpus "$dm_threads" --pids-limit 256 \
  -e HOME=/tmp -e PYTHONDONTWRITEBYTECODE=1 -e OMP_NUM_THREADS="$dm_threads" "${env_args[@]}" \
  "${tool_mounts[@]}" \
  --mount type=bind,src="$out/prep/run/prepared",dst=/prep/prepared,readonly \
  --mount type=bind,src="$out/prep/summary-extra.json",dst=/prep/summary-extra.json,readonly \
  --mount type=bind,src="$out/unit",dst=/out \
  "${mounts[@]}" --entrypoint python3 -w /tools "$dm_image" /tools/router_driver.py \
  "${model_args[@]}" --threads "$dm_threads" --n-ctx "$dm_n_ctx" \
  --states /prep/prepared/states.jsonl --plan "/prep/prepared/plan-$unit.json" --out /out \
  --ready-timeout "$ready_timeout" --decision-timeout "$decision_timeout" \
  --deadline-epoch "$((deadline - WINDOW_MARGIN))" --max-restarts "$max_restarts" --summary-extra /prep/summary-extra.json \
  >/dev/null || fail 12 infra_docker "router container did not start"
while :; do
  status=$(dk inspect -f '{{.State.Status}} {{.State.ExitCode}} {{.State.OOMKilled}}' "$ROUTER") \
    || fail 12 infra_docker "docker inspect of the router failed"
  case $status in exited*|dead*) break ;; esac
  still_allowed "router unit"
  temps >/dev/null
  sleep "$POLL_SECONDS"
done
read -r _ main_rc oom <<< "$status"
{
  echo "main_exit=$main_rc main_oom_killed=$oom main_wall_seconds=$(( $(date +%s) - t0 ))"
  echo "ledger_file=unit/decisions.jsonl summary_file=unit/summary.json"
} >> "$prov"
[ "$main_rc" = 0 ] || exit "$main_rc"
