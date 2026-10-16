# Tool-loop benchmark (prototype tier)

Status: **harness v2 (2026-10-15).** The first real runs used harness v1. Their results are in
[tool-loop-benchmark-results-2026-10-15.md](tool-loop-benchmark-results-2026-10-15.md), which
v2 supersedes for scoring (see its "Harness v2" section). v2 has been checked with unit tests, a
differential formula replay, scripted dry runs and one 3-case live smoke; see
[Verification](#verification). It addresses B1, B2, B3, B5, B6 and B8 from
[harness-assessment-2026-10-15.md](harness-assessment-2026-10-15.md). B4 (sample size) and B7
(corpus size) depend on the case set and the pack, not on this code.

Files:

- `tools/tool_loop_benchmark.py`: host, tool loop, scoring and CLI. It uses only the
  standard library (sqlite3 FTS5, subprocess).
- Formulas are CEL profile `cel-mimik-1`, evaluated by the Rust `formula_eval` JSON-lines
  CLI (`examples/formula_eval.rs`) through the `FormulaEngine` seam (`CelCliEngine`). Python
  has no formula semantics: the custom-spec `tools/formula_ref.py` and its tests were deleted
  after the differential check below.
- `tools/test_tool_loop_benchmark.py`: host, loop and scoring tests. They use the real CLI
  when it is built and otherwise `FixtureEngine`, a test double that knows only the fixture's
  four CEL strings.
- `tools/test_cel_engine.py`: `CelCliEngine` against the CLI. Like master's
  `tools/test_formula_cli.py`, it **fails closed** when the binary is missing.
- `tools/fixtures/tool_loop_pack/`: a small original fixture pack (CEL) plus
  `dry-run-script.json`.

## How to run

Build the CLI once, in a container, into the `mimik-formula-target` volume:

```sh
docker run --rm --network none --cpus 4 \
  --mount type=bind,src=$PWD,dst=/workspace,readonly -w /workspace \
  --mount type=volume,src=mimik-cargo-cache,dst=/opt/cargo/registry \
  --mount type=volume,src=mimik-formula-target,dst=/target \
  -e CARGO_TARGET_DIR=/target/harness-formula-eval mimik-ci:latest \
  cargo build --release --locked --example formula_eval
```

The binary is `/target/harness-formula-eval/release/examples/formula_eval` inside that volume.
Mount the volume read-only into the benchmark container and pass `--formula-cli` (or set
`MIMIK_FORMULA_EVAL`). Its sha256 and `version` response (profile, `cel 0.15.0`, conformance
fixture hash) are written to every ledger header under `formula_engine`.

```sh
F=/formula/harness-formula-eval/release/examples/formula_eval
RUN="docker run --rm --network none --user 1000:1000 --cap-drop ALL --security-opt no-new-privileges
  --read-only --tmpfs /tmp --mount type=bind,src=$PWD/tools,dst=/tools,readonly
  --mount type=bind,src=$PWD/examples/tool-loop-demo,dst=/pack,readonly
  --mount type=volume,src=mimik-formula-target,dst=/formula,readonly
  --mount type=bind,src=$PWD/target,dst=/out mimik-toolloop-py:20261015"
$RUN python3 /tools/tool_loop_benchmark.py --pack /pack --validate-pack --formula-cli $F
$RUN python3 /tools/tool_loop_benchmark.py --pack /pack --split dev --model-label fake \
  --seeds 17,29 --dry-run --formula-cli $F --output /out/tool-loop-dry.jsonl
python3 tools/tool_loop_benchmark.py --summarize target/tool-loop-*.jsonl   # inside mimik-ci
```

**Canonical live runner (one documented command):**

```sh
tools/run_tool_loop_benchmark.sh <qwen35-9b|qwen3-4b-2507|qwen36-35b-a3b> <smoke|dev|heldout> [seeds] [ctx]
# e.g. the 35B MoE model (experts in system RAM, required):
N_CPU_MOE=<layers> tools/run_tool_loop_benchmark.sh qwen36-35b-a3b heldout 17,29
```

- **Output** goes to `target/tool-loop-benchmark-<date>/<model-key>/<split>-<time>/`:
  `provenance.txt`, `server.log`, `props.json`, `harness.stdout`, `harness.stderr` and the
  ledger. `ROOT` is derived from the script location.
- **Environment:** `BUDGET_SECONDS` (default 0 = none), `N_CPU_MOE` (passed as
  `--n-cpu-moe`; required for `qwen36-35b-a3b`), `SRV_MEMORY` (server container limit;
  default 14g, or 26g for the 35B model), `MODEL_PATH` / `MODEL_SHA256` (override the model
  file and its expected hash), `FORMULA_CLI_SHA256`, `CLI_BUILD_INFO` and `CHECK_ONLY=1`
  (stop after the pre-launch checks).
- **Smoke** is the dev split restricted with `--cases dev-001,dev-005,dev-008`.
- **Containment:** the server and harness run on an internal Docker network with no
  published ports, as uid 1000, with `--cap-drop ALL`, no-new-privileges and read-only root
  filesystems. Llama.cpp is a pinned image; the harness and check images are pinned by local
  image id.
- **Strictly sequential runs:** one GPU; a lock file in `target/` refuses a second run (exit
  9).
- **Pre-launch checks:** the model and the CLI are hashed in a locked-down container
  (`--network none`, uid 1000, `--cap-drop ALL`, read-only) and the run aborts on mismatch
  (exit 5 and 3).
- **Provenance:** the actual hashes, the `tools/tool_loop_benchmark.py` sha256, git HEAD and
  dirty state, and the CLI source commit are written to `provenance.txt` and to the ledger
  header (`model_sha256`, `runner_provenance`, `harness_sha256`).
- **Prompt cache** is on: the server has no `--no-cache-prompt`, the harness passes
  `--cache-prompt`, and `prompt_cache=on` is recorded.
- The v1 runner `target/tool-loop-benchmark-20261015/run.sh` is kept only as v1 evidence.

Unit tests (the CLI must be built; set `MIMIK_FORMULA_EVAL` to it):

```sh
docker run --rm --mount type=bind,src=$PWD,dst=/workspace \
  --mount type=volume,src=mimik-formula-target,dst=/target,readonly \
  -e MIMIK_FORMULA_EVAL=/target/harness-formula-eval/release/examples/formula_eval mimik-ci \
  bash -c 'cd /workspace && ruff check tools && ruff format --check tools && python -m unittest discover -s tools -p "test_*.py"'
```

Options: `--temperature` (default 0.1), `--top-p` (0.95), `--max-tokens` per turn (**2048**),
`--max-turns` (8, also the maximum), `--top-k` retrieval (5), `--tool-choice auto|required`,
`--thinking off|on|default` (sends `chat_template_kwargs.enable_thinking` unless `default`),
`--cache-prompt` (**default on**; use `--no-cache-prompt` to disable; the value is recorded in
the ledger's `sampling`), `--request-timeout` (300 s, a SIGALRM wall-clock deadline),
`--budget-seconds` (0 means no budget) and `--formula-cli`. Ledgers are opened exclusively, so
an existing file is never overwritten. The endpoint must be plain `http://host[:port]`.

## Episode protocol (v2)

**Context.** The model receives a short, game-agnostic system prompt and one user message of
JSON data:

- the request;
- fields, each with `path`, `type`, enum `values`, `editable`, `derived` and current value;
- collections, each with `entry_path` (`<collection>/<entry_id>/<field>`), the entry field
  schema and the entry ids and names;
- the current revision;
- any pending proposal, in path form.

Rules are not pre-stuffed. Only `id`, `character`, `request` and `setup` of a case reach the
host (`public_case`).

**One path syntax** applies to every tool: `"<field>"` or `"<collection>/<entry_id>/<field>"`.
`read_fields` also accepts `"<collection>"` and `"<collection>/<entry_id>"`. Pack files keep
their `{collection?, entry_id?, field, to}` change format, which the host converts to paths.

| Tool | Behaviour |
|---|---|
| `search_rules(query)` | SQLite FTS5 (`unicode61 remove_diacritics 2`, no Porter stemming), bm25 order with rule id tie-break, top-k 5. The query becomes ≤ 32 quoted `\w+` tokens joined with OR, so no FTS syntax injection. Every returned id is recorded. |
| `read_fields(paths)` | Current values. Unknown paths are rejected with `valid_paths`. |
| `propose_change(changes:[{path,to}], rule_ids, reason)` | The host rejects: bad or unknown paths; derived fields (with an explanation); non-editable fields; type mismatches (integer excludes booleans, enum must be a listed value); duplicates; no-ops; and rule_ids not returned by `search_rules` in this episode. Path errors return **`valid_paths`** (the editable paths). The v1 `{collection, entry_id, field}` shape is rejected with the same actionable message. On success the host fills old values and the base revision, has the CEL engine derive fields and evaluate constraints on the proposed state, and returns `{base_revision, changes, derived, violations}`. Nothing is applied. |
| `submit_proposal(rule_ids, reason)` | **The only way to end with a proposal**: it submits the last accepted preview. With no accepted preview it is a counted rejection, and the episode continues. |
| `answer(message, values, rule_ids)` | Ends with `answer`. **`values` is required**: a non-empty object of name → integer/boolean/string. Missing or empty values make the call malformed. |
| `respond(kind, message, rule_ids)` | Ends with `conflict`, `stale` or `refuse`. |
| `ask_user(kind, question)` | Ends with `ask_rule` or `ask_fact`. |

- **No implicit outcomes.** A reply without tool calls is `no_outcome`, even when an accepted
  proposal is pending (`pending_proposal_at_end` is set). Tool-like JSON written as plain text
  is never parsed.
- After 8 model turns without a finishing call the outcome is `timeout`.
- Exceptions (HTTP errors, deadlines, bad response shapes, formula CLI failures) give
  `error`. Episodes skipped by the budget are written as `not_run`.
- Calls after the finishing call in the same message are ignored and counted
  (`calls_after_terminal`).
- **Malformed calls** are returned as errors and counted in `malformed_calls`; they are never
  repaired. Examples: an unknown tool, invalid JSON, missing, extra or wrongly typed arguments,
  an invalid `kind`, or `answer` without values. Semantic rejections are counted in
  `rejected_calls`.

**Pack checks on load:**

- the definition needs `formula_profile: 1` and `formula_language: "cel-mimik-1"`, and must
  compile in the CLI;
- stored derived values must equal what the CLI derives;
- overriding a derived field is an error;
- expected and pending changes must pass the same validation as `propose_change`;
- answer cases need `answer_values`;
- gold and constraint rule ids must exist.

**Formula infrastructure faults fail closed.** If the CLI exits, closes its output, times
out (30 s), breaks the pipe, sends non-JSON or returns a protocol error, the harness raises
`FormulaInfraError`. This is never shown to the model: the run writes a `run_aborted` row with
a bounded stderr tail (≤ 8 KiB; CLI stderr goes to a temp file, never `DEVNULL`) and exits
with code 3. Only coded admission/evaluation errors (`FormulaEvalError`) go back to the model
as proposal rejections, from `derive` and `violations` alike.

**Fail fast.** After 5 consecutive `error` episodes, or after an error episode followed by a
failed `GET /health`, the run writes `run_aborted` and exits with code 4.

**Job order.** Within each seed, cases follow a deterministic, seed-independent interleave by
expected outcome. Each case takes the fractional position (i + 0.5)/n in its outcome group, in
file order, so any budget cut keeps outcome proportions; the order is in the ledger header
(`case_order`). The expected outcome is used only for ordering and never enters the context.

**`--summarize` refuses** to merge ledgers that differ in harness version, pack hashes or
formula CLI sha256. It also refuses ledgers for the same `model_label` whose `model_sha256`
differs, and any ledger that was aborted or has no header.

Constraint results are tri-state from the CLI: `pass`, `violated` or `error(code)`. Both
`violated` and `error` are reported as violations, with `status`.

## Scoring (per episode)

| Metric | Definition |
|---|---|
| `retrieval_recall` | \|gold ∩ ids returned by any search\| / \|gold\|. n/a when gold is empty, and None for `not_run` |
| `outcome_correct` | final outcome == expected outcome |
| `changes_exact` | proposal cases only: the set of submitted changes `(collection, entry_id, field, typed value)` equals the expected set, ignoring order |
| `wrong_proposal` | a submitted proposal that is wrong: expected proposal with inexact changes, **or any submitted proposal when the expected outcome is not a proposal** (review 2026-10-15; at-risk cases also get `outcome_error`). The reverse, `answer` where a proposal was expected, is incorrect but **not** false support (documented decision) |
| `answer_check` | answer cases answered with `answer`, **scored by typed value, ignoring keys**. `verified_correct`: every expected value appears among the reported `values` (typed multiset), no reported boolean or string contradicts the expected values of that type (hedge guard: `{x: true, y: false}` against expected `{can_cross: false}` is wrong), and at most len(expected)+1 values were given. `wrong` (false support, `wrong_answer`): an expected value is missing or hedged. `over_cap` (false support, `over_cap`): more than len(expected)+1 values. `unverified` occurs only in v1 ledgers |
| `answer_values_key_exact`, `answer_values_key_mismatch` | diagnostics only, reported beside `correct`: every expected key present with a typed-equal value; or each value missing under its key appears under another key (e.g. `watches` for `travel_watches`) |
| `correct` | `outcome_correct` ∧ `changes_exact` (proposal cases) ∧ `answer_check == verified_correct` (answer cases) ∧ ¬`false_supported` |
| `citation_valid` | cited ⊆ retrieved. For proposals, cited = the accepted preview's ids ∪ the `submit_proposal` ids; otherwise the `answer`/`respond` ids; n/a for ask, timeout and error |
| `citation_covers_gold` | gold ⊆ cited; n/a when gold is empty or nothing was cited |
| `false_supported` | any of `outcome_error` (proposal/answer while expected ask_rule/ask_fact/conflict/stale/refuse), `unreported_violation` (a submitted preview with violations), `wrong_proposal`, `wrong_answer`, `over_cap` (answer value dump) |

Each episode also records:

- tool calls, malformed and rejected calls, and turns;
- per-request latency, prompt and completion tokens, and finish reasons;
- raw responses and the full transcript.

The ledger header records pack file hashes, the corpus hash, the system-prompt and tool-schema
hashes, sampling (including `cache_prompt` and `max_tokens`), the planned episode count and
`formula_engine` provenance.

**Typing is strict everywhere:** `"4"` is not `4`, `true` is not `1`, and strings (text and
enum values) compare exactly and case-sensitively (`"Mail"` ≠ `"mail"`). The bar verdict below
is computed with exactly these rules: value-based answers with the hedge guard and the
len+1 cap, and `wrong_proposal` for any unexpected proposal.

**Summary**, per `model_label × split`:

- Rates use all planned episodes (including `not_run`) as the denominator, unless a metric
  states its own.
- **Retrieval recall** is the mean over episodes that ran and have gold; `not_run` is
  reported beside it, never counted as 0.
- **False support** is reported over all episodes and over at-risk episodes (expected
  ask_rule, ask_fact, conflict, stale or refuse), with per-category and per-expected-outcome
  counts.
- **Bar** (user decision): held-out, `correct ≥ 90%` and `false_supported ≤ 2%`, at least 100
  unique cases and no `not_run`. It is labelled **"bar: pending denominator choice"**, with
  PASS/FAIL per variant (`all_cases`, `at_risk_cases`). An empty denominator fails.

## Limitations

- A model that writes a finishing call as plain text gets `no_outcome`. This is deliberate,
  because the product will not parse prose into actions, but it means template/parser fit
  dominates small-model scores.
- The prose of conflict, ask, refuse and answer messages is not graded. Answer correctness
  is the typed `values` only, so a correct value with a wrong explanation still counts as
  correct.
- Retrieval is lexical FTS5 only (no vectors or reranker). Italian queries do not match
  English rule text.
- A formula evaluation error during a proposal rejects it. Constraint errors are reported as
  violations.
- There is no way to add or remove collection entries or to set an optional property to
  null.
- Results with fewer than 100 held-out cases, or one seed, cannot rank models (B4).

## Formula engine (CEL) and differential check

The user decision is CEL via the Rust `cel` crate, plan §8/§11 in
[derived-formulas-plan.md](derived-formulas-plan.md). `examples/tool-loop-demo/definition.json`
now uses the exact plan §8 CEL strings, taken verbatim from the lantern sheets of
`schemas/formula-conformance-v1.json`, and carries `formula_profile: 1` and
`formula_language: "cel-mimik-1"`. The harness's seam is `FormulaEngine` / `CelCliEngine` in
`tools/tool_loop_benchmark.py`.

**Differential replay** (one-time, before deleting `formula_ref.py`). Every dev and held-out
case went through `formula_ref` on the v1 custom-spec definition and through the CLI
(sha256 `f0e13322…fc10`) on the CEL definition. The replay covered three kinds of state:

- each case's initial state;
- the state after its expected changes;
- the state after its pending proposal, where it has one.

That is 249 states from 174 cases. **6,485 boundary perturbations** of every editable path of
the 25 distinct initial states were added (integers −1/0/1/3/4/17/99, both booleans, every
enum option). 2,606 of these states violate at least one constraint, and 21 have a derived-field
error.

Results:

- **0 mismatches** over 74,074 derived-field and 140,973 constraint comparisons (value,
  error-ness, and pass/violated/error).
- Error codes differ only in name: CEL `missing_property` versus ref `missing_value` (28),
  and `dependency_failed` in both (14).
- In the 21 error states, constraints were not compared, because `formula_ref` produces no
  constraint results without derived values.

Script, output and the v1 definition are kept locally in
`target/tool-loop-benchmark-20261015/harness-v2/`.

## Verification

See the results doc's "Harness v2" section for the run details. In summary, on 2026-10-15 in
`mimik-ci`, with the CLI mounted:

- **Unit tests:** ruff check, ruff format and unittest all passed: 181 tests across `tools/`,
  including other agents' suites.
  - Failing-first regressions, each run against the v1 code first, then fixed:
    - path syntax shared with `read_fields`;
    - actionable path errors with `valid_paths`;
    - no implicit proposal from a plain reply or from plain-text JSON;
    - explicit `submit_proposal`;
    - required `answer` values;
    - defaults (cache on, 2048 tokens);
    - recall that excludes `not_run`;
    - `CelCliEngine.close()` leaving its stdout pipe open.
  - Without the binary, `test_formula_cli` (master) and `test_cel_engine` fail closed by
    design.
- **Pack checks:** `--validate-pack` passed on `examples/tool-loop-demo` (44 rules, 4
  characters, 33 dev and 141 held-out cases).
- **Dry runs:**
  - A trivial scripted dry run (search, then answer) finished 66 dev and 282 held-out
    episodes with no errors.
  - A scripted replay of every case's expected outcome through the full loop and CLI scored
    33/33 dev and 141/141 held-out with 0 errors. It checks the harness only, not a model.
- **Live smoke:** 9B on 3 dev cases (see the results doc). `submit_proposal`, `answer` and
  `respond` were all called explicitly, with 0 malformed calls. The dev-005 key mismatch
  (`watches` vs `travel_watches`) led to value-based answer scoring; a regression test
  covers it.

Self-check under the prototype tier. Independent review is required before results drive a
product decision.
