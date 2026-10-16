#!/usr/bin/env bash
# Launch tool-loop queues from a frozen, detached git worktree (second review round, N1).
# Prototype tier. Edits to any working checkout cannot affect a running queue, and the
# harness sha pinned in each ledger is the frozen commit's by construction. The runner,
# queue and watchdog locks stay shared in the main checkout's target/ (git common dir).
#
# Usage: tools/launch_frozen_queue.sh [--print] [--session-end "YYYY-MM-DD HH:MM"] <commit>
#          <queue.json> [<queue.json>...]
#   commit      any git revision; resolved to a full sha
#   queue.json  paths relative to the repository root, run in order (one queue at a time)
#   --print     create and check the worktree, print the launch command, do not start it
#   --session-end  absolute local end passed to every queue: no arm starts or runs past it
# Environment: FROZEN_BASE (default <main checkout>/../mimik-wt) holds run-<12-hex>/.
# Refuses MIMIK_MODEL_ROOT (models come from the registry default), DOCKER_HOST,
# DOCKER_CONTEXT, DOCKER_CONFIG or a non-default docker context (one Docker endpoint), and any
# running run_queue.py or run_watchdog.py process.
set -euo pipefail
die() { echo "$1" >&2; exit 2; }
print_only=0 session_end=''
while [ $# -gt 0 ]; do
  case $1 in
    --print) print_only=1; shift ;;
    --session-end) [ $# -ge 2 ] || die "--session-end needs a value"; session_end=$2; shift 2 ;;
    *) break ;;
  esac
done
[ $# -ge 2 ] || { sed -n '7,12p' "$0" >&2; exit 2; }
if [ -n "$session_end" ]; then
  if ! [[ $session_end =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}\ [0-9]{2}:[0-9]{2}$ ]] \
    || ! date -d "$session_end" >/dev/null 2>&1; then
    die "--session-end must be 'YYYY-MM-DD HH:MM', got '$session_end'"
  fi
  [ "$(date -d "$session_end" +%s)" -gt "$(date +%s)" ] || die "--session-end is in the past"
fi
for v in MIMIK_MODEL_ROOT DOCKER_HOST DOCKER_CONTEXT DOCKER_CONFIG; do
  [ -z "${!v:-}" ] || die "refusing to launch with $v set (unset it: registry root, one Docker endpoint)"
done
docker_context=$(timeout --foreground -k 5 60 docker context show 2>/dev/null) \
  || die "docker context show failed"
[ "$docker_context" = default ] || die "refusing docker context '$docker_context' (default only)"
# Only real interpreter processes (python ... run_queue.py / run_watchdog.py), not any command
# line that merely mentions the file names.
if pgrep -f '^[^ ]*python[0-9.]* [^ ]*run_(queue|watchdog)\.py( |$)' >/dev/null 2>&1; then
  die "a run_queue.py or run_watchdog.py process is already running; stop it first"
fi
ROOT=$(cd "$(dirname "$0")/.." && pwd)
MAIN=$(dirname "$(timeout 30 git -C "$ROOT" rev-parse --path-format=absolute --git-common-dir)")
commit=$(timeout 30 git -C "$ROOT" rev-parse --verify "$1^{commit}") || die "unknown commit $1"
shift
base=${FROZEN_BASE:-$(dirname "$MAIN")/mimik-wt}
wt=$base/run-${commit:0:12}
timeout 60 git -C "$MAIN" worktree prune
if [ ! -e "$wt" ]; then
  mkdir -p "$base"
  timeout 120 git -C "$MAIN" worktree add --detach "$wt" "$commit" >/dev/null
fi
[ "$(timeout 30 git -C "$wt" rev-parse HEAD)" = "$commit" ] || die "$wt is not at $commit"
[ -z "$(timeout 30 git -C "$wt" status --porcelain --untracked-files=no)" ] \
  || die "$wt has local changes; a frozen copy must be clean"
queues=()
for q in "$@"; do
  case $q in /*|*..*) die "queue paths must be relative to the repository root: $q" ;; esac
  [ -f "$wt/$q" ] || die "queue file $q is not in commit $commit"
  queues+=("$wt/$q")
done
mkdir -p "$wt/target"
script=''
end_args=''
[ -z "$session_end" ] || end_args=" --session-end $(printf '%q' "$session_end")"
for q in "${queues[@]}"; do
  script+="python3 $(printf '%q' "$wt/tools/run_queue.py") run $(printf '%q' "$q")$end_args; "
done
console=$wt/target/queue-$(date +%Y%m%dT%H%M%S).console
echo "frozen_worktree=$wt commit=$commit"
echo "command: setsid nohup bash -c '$script' > $console 2>&1 < /dev/null &"
[ "$print_only" = 0 ] || exit 0
cd "$wt"
setsid nohup bash -c "$script" > "$console" 2>&1 < /dev/null &
echo "started pid=$! console=$console"
