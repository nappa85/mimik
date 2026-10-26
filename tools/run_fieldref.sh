#!/usr/bin/env bash
# Field-reference experiment runner (prototype tier; decision rule: tools/fieldref-prereg.md).
# Granite 4.2 3B Q4_K_M on the CPU, two arms (base = adopted v2 prefix, compact = v2 plus the
# compact-reply sentence), pass 1 only, 5 threads (ASSIST-41 here), over the 30 cases of
# tools/assist-eval/fieldref-cases.json. Unattended, never reads input.
#
# Usage: tools/run_fieldref.sh [out-dir]   (default /media/data/mimik/fieldref/runs/<stamp>)
# Refuses on a weekday from 09:00 to 18:00 and when the host is busy; no request starts at
# or after the next weekday 09:00 (a request already running then finishes). Environment:
# LOAD_MAX (1.0: limit for the 1- and 5-minute load averages, recorded in run.log);
# OWNER_WINDOW_OVERRIDE=<today's date, YYYY-MM-DD> lifts the weekday refusal for that day only
# (an explicit owner exception, recorded in run.log).
set -euo pipefail
export TZ=:/etc/localtime
ROOT=$(cd "$(dirname "$0")/.." && pwd)
OUT=${1:-/media/data/mimik/fieldref/runs/$(date +%Y%m%d-%H%M)}
LOAD_MAX=${LOAD_MAX:-1.0}
THREADS=5
CPUS=0,1,2,3,4
now=$(date +%s)
day=$(date +%u); hour=$((10#$(date +%H)))
override=""
if [ "${OWNER_WINDOW_OVERRIDE:-}" = "$(date +%F)" ]; then override=" (owner window override $OWNER_WINDOW_OVERRIDE)"; fi
if [ -z "$override" ] && [ "$day" -le 5 ] && [ "$hour" -ge 9 ] && [ "$hour" -lt 18 ]; then
  echo "weekday 09:00-18:00: no heavy runs" >&2; exit 2
fi
# The next weekday 09:00 after now (today's, if now is a weekday before 09:00).
deadline=""
for offset in 0 1 2 3 4 5 6 7; do
  cand=$(date -d "$(date -d "+$offset day" +%F) 09:00" +%s)
  dow=$(date -d "@$cand" +%u)
  if [ "$dow" -le 5 ] && [ "$cand" -gt "$now" ]; then deadline=$cand; break; fi
done
[ -n "$deadline" ] || { echo "no deadline found" >&2; exit 2; }
read -r l1 l5 _ < /proc/loadavg
if ! awk -v a="$l1" -v b="$l5" -v m="$LOAD_MAX" 'BEGIN{exit !(a<=m && b<=m)}'; then
  echo "host not idle: load $l1 $l5 > $LOAD_MAX" >&2; exit 3
fi
if pgrep -x llama-server >/dev/null; then echo "a llama-server is running" >&2; exit 3; fi
[ ! -e "$OUT" ] || { echo "$OUT exists" >&2; exit 2; }
mkdir -p "$OUT"
echo "$(date '+%F %T') start: load $l1 $l5 (max $LOAD_MAX), threads $THREADS, cpus $CPUS, deadline $(date -d "@$deadline" '+%F %T')$override" | tee -a "$OUT/run.log"
invalid=0
for arm in base compact; do
  case $arm in base) contract=v2 ;; compact) contract=v2-compact ;; esac
  CONTRACT=$contract PRIME=1 PASSES=1 STRESS=0 COLD=1 COLD_ONLY='' THREADS=$THREADS CPUS=$CPUS \
    CASES=assist-eval/fieldref-cases.json MAX_TOKENS=352 NAME=fieldref DEADLINE_EPOCH=$deadline \
    "$ROOT/tools/run_assist_screen.sh" "$OUT/$arm" granite42-3b </dev/null \
    || echo "$(date '+%F %T') $arm: runner exit $?" | tee -a "$OUT/run.log"
  if [ -s "$OUT/$arm/granite42-3b/screen.jsonl" ]; then
    python3 -I "$ROOT/tools/assist_eval.py" summarize "$OUT/$arm/granite42-3b/screen.jsonl" \
      > "$OUT/summary-$arm.json" || echo "$arm: summary failed" | tee -a "$OUT/run.log"
    python3 -I "$ROOT/tools/assist_eval.py" fieldref-summary "$OUT/$arm/granite42-3b/screen.jsonl" \
      > "$OUT/fieldref-$arm.json" || { echo "$(date '+%F %T') $arm: INVALID run (not all 30 cases completed)" | tee -a "$OUT/run.log"; invalid=1; }
  else
    echo "$(date '+%F %T') $arm: INVALID run (no screen ledger)" | tee -a "$OUT/run.log"; invalid=1
  fi
done
[ "$invalid" = 0 ] || { echo "$(date '+%F %T') INVALID: repeat the whole run" | tee -a "$OUT/run.log"; exit 1; }
echo "$(date '+%F %T') done; results: $OUT/fieldref-*.json" | tee -a "$OUT/run.log"
