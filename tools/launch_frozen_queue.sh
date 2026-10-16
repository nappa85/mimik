#!/usr/bin/env bash
# Launch tool-loop queues from a frozen, detached git worktree (second review round, N1).
# Prototype tier. Edits to any working checkout cannot affect a running queue, and the
# harness sha pinned in each ledger is the frozen commit's by construction. The runner,
# queue and watchdog locks stay shared in the main checkout's target/ (git common dir).
#
# Usage: tools/launch_frozen_queue.sh [--print] <commit> <queue.json> [<queue.json>...]
#   commit      any git revision; resolved to a full sha
#   queue.json  paths relative to the repository root, run in order (one queue at a time)
#   --print     create and check the worktree, print the launch command, do not start it
# Environment: FROZEN_BASE (default <main checkout>/../mimik-wt) holds run-<12-hex>/.
# Refuses DOCKER_HOST, DOCKER_CONTEXT or DOCKER_CONFIG (one Docker endpoint only).
set -euo pipefail
die() { echo "$1" >&2; exit 2; }
print_only=0
if [ "${1:-}" = --print ]; then print_only=1; shift; fi
[ $# -ge 2 ] || { sed -n '7,11p' "$0" >&2; exit 2; }
for v in DOCKER_HOST DOCKER_CONTEXT DOCKER_CONFIG; do
  [ -z "${!v:-}" ] || die "refusing to launch with $v set (one Docker endpoint only)"
done
ROOT=$(cd "$(dirname "$0")/.." && pwd)
MAIN=$(dirname "$(timeout 30 git -C "$ROOT" rev-parse --path-format=absolute --git-common-dir)")
commit=$(timeout 30 git -C "$ROOT" rev-parse --verify "$1^{commit}") || die "unknown commit $1"
shift
base=${FROZEN_BASE:-$(dirname "$MAIN")/mimik-wt}
wt=$base/run-${commit:0:12}
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
for q in "${queues[@]}"; do script+="python3 $(printf '%q' "$wt/tools/run_queue.py") run $(printf '%q' "$q"); "; done
console=$wt/target/queue-$(date +%Y%m%dT%H%M%S).console
echo "frozen_worktree=$wt commit=$commit"
echo "command: setsid nohup bash -c '$script' > $console 2>&1 < /dev/null &"
[ "$print_only" = 0 ] || exit 0
cd "$wt"
setsid nohup bash -c "$script" > "$console" 2>&1 < /dev/null &
echo "started pid=$! console=$console"
