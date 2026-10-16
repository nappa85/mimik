# Review record: tool-loop benchmark harness v2 results

2026-10-16. Scope: [results doc](../tool-loop-benchmark-results-2026-10-15.md), sections
"Harness v2 results" and "Harness v2 failure analysis". The failure analysis was written
by a read-only Opus subagent.

## Independence

There were three reviewers in parallel (user policy for 2026-10-16), working on a sanitized
snapshot that also contained both ledgers, `harness.stdout` and `provenance.txt`:

- **Sonnet 5.5**, a fresh subagent. It recomputed numbers with stdlib Python inside the
  snapshot.
- **Muse Spark 1.3** and **Space Bunny**, via opencode. They are read-only and cannot run
  code, so they can only read and grep.

The main session verified the disputed points from the ledgers and the code. Prototype
tier: these results may now inform the next experiment, but no product decision.

## Verdicts

All three returned **SOUND-with-corrections**. Severity labels disagree: Sonnet marked
nothing Blocking, Muse 5 and Space Bunny 3. The corrections are recorded as binding in the
results doc, under "Results review corrections" items 1–11.

| Finding | Raised by | Main-session check | Disposition |
|---|---|---|---|
| Both-seeds false-supported is 26/141, not 21 | Sonnet | Recomputed: 26 (4B: 18) | Fixed (item 1). Muse repeated the wrong 21 from the doc |
| `answer`-as-abstain (14) drives 20% of false-supported; report it both ways | all three | 56/282, 32/112 | Fixed (item 2) |
| v3 estimate arithmetic wrong; F2 raises false-supported | all three | Components sum to 67–74 | Fixed (item 6) |
| Held-out contaminated by reading transcripts | Sonnet, Space Bunny | Accepted | Fixed (item 7): fresh held-out needed for v3 |
| "Typed actions biggest lever" unsupported | Sonnet, Muse | ≤13/70 false-supported addressable | Reworded (item 8) |
| Bar not reached in this configuration only; untested levers | Sonnet, Muse, Space Bunny | Accepted | Reworded (item 8) |
| At-risk correct rate never reported | Space Bunny | 42/112 (9B), 25/112 (4B) | Added (item 3). The "bug" framing was rejected: the all-case correct criterion is by design |
| Token-limit episodes misassigned (5 end on `length`) | Space Bunny | Five confirmed | Fixed (item 4) |
| ho-118: echo is protocol-correct; upstream robustness gap | Sonnet (parser, not Jinja), Space Bunny | Accepted | Reworded (item 5) |
| Scoring leniencies (single bool, int hedge, empty citations, answer-on-proposal, `no_outcome`) | all three | Code read | Disclosed (item 9) |
| Provenance: latency source, 4B build overlap, load split, head diff | Muse, Space Bunny, Sonnet | Head diff touches no harness input | Fixed (item 10) |
| No v2 limitations section | Space Bunny | Accepted | Added (item 11) |
| `wrong_proposal` summary rate uses only expected-proposal cases | Muse | `summarize()` reports 17/102; the total of 41 is in the reasons | Noted for v3 reporting; not a scoring bug |
| Stale gold partly model error, not pack ambiguity | Sonnet, Muse | Prompt vs LR2-3 vs case notes conflict | Bounded range; fix the pack before v3 |
| Dara-stratified table needed | Muse | Accepted | v3 reporting |
| `MAX_TURNS` hard-coded | Space Bunny | `tools/tool_loop_benchmark.py:41` | v3 parameter |

Wrong claims by reviewers:

- **Muse:**
  - the seed-agreement figures (it repeated the doc's error);
  - "ho-118 harness-defect claim unproven": the ledger has the parse error, as Space Bunny
    quoted.
- **Space Bunny:** "at-risk bar silently reuses the all-case correct rate" framed as a bug.
  It is the user's bar by design; the reporting gap it pointed at is real.

## Reviewer benchmark note

See [reviewer benchmark](../reviewer-benchmark.md) experiment 2.

- **Sonnet** was the only reviewer able to recompute the numbers, and it was the only one
  to catch the seed-agreement error.
- **Space Bunny** produced the most precise ledger line citations and the most unique valid
  catches: the at-risk correct rate, the token-limit episodes and missing limitations.
- **Muse** was the fastest and largely duplicated the others. It labelled five findings
  Blocking, which inflates their severity.

Review findings are not proof of correctness.
