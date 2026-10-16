# Reviewer benchmark

**Concluded 2026-10-16 (user decision).** The standing setup is in `AGENTS.md`:
- main reviewers: Sol and Sonnet in parallel;
- third reviewer: the first that works of Space Bunny, DeepSeek and Muse Spark
  (`tools/external_review.py --chain third`); if none works, proceed with two.

No further comparison runs are planned. The record below stays as evidence.

A cumulative record of how candidate review models perform on real Mimik review tasks. The
user asked for it on 2026-10-16 and keeps it separate from the per-change review records in
`docs/reviews/`. Review policy for 2026-10-16: Muse Spark, Space Bunny and Sonnet review in
parallel. The experiment is being expanded to every opencode free model.

## Method

- **Same input for all:** a frozen, sanitized snapshot, built by `tools/external_review.py`
  (no `.env`, `data/`, `target/`, manuals or `.git`), and an identical prompt.
- opencode models run as the read-only `reviewer` agent, which has only
  `read`/`glob`/`grep`/`todowrite`. Sonnet runs as a fresh read-only subagent told to stay
  inside the snapshot.
- **Scoring:** before any new model's output is read, every finding is mapped to the fixed
  answer key of the experiment. Each finding is classed as:
  - a **valid** key issue (with its key ID);
  - **valid-new** (a real issue missing from the key, which is added for later models);
  - a **duplicate** within the same review;
  - **wrong** (contradicted by the code);
  - **harmful** (following it would break a gate or weaken a protection).
- **Per model:** wall time; findings; key issues hit; weighted recall, where Blocking-worthy
  key issues count double; wrong and harmful counts; unique catches; citation accuracy;
  severity calibration.
- **Limits:** the main session adjudicates alone; there is one task per experiment and one
  run per model unless stated. Run-to-run variance is large: the two Sonnet runs on the same
  input overlap by about half. Free tiers may log or train on prompts, so only repo source
  and docs are sent.

# Experiment 1: M3 plan review (2026-10-16)

## Setup

- **Input:** a frozen, sanitized snapshot of the repo with
  `docs/derived-formulas-m3-plan.md` as it was before review amendments (§10 and the first
  review record removed). One artefact of staging: step 6 still referenced "§10.5"; findings
  about that dangling reference are ignored.
- **Prompt:** identical text for Muse, Space Bunny and Sonnet B. Sonnet A is the earlier
  review of the same plan content with a longer, differently worded brief.
- **Runs (parallel):**
  - Muse Spark 1.3 (`opencode/muse-spark-1.3-contributor-free`);
  - Space Bunny (`opencode/space-bunny-free`);
  - Sonnet B (fresh Sonnet 5.5 subagent).
  - The opencode runs use the read-only `reviewer` agent via `tools/external_review.py`.
- **Adjudication:** the main session (Opus 5.5) checked every disputed finding against the
  code. The adjudication is itself single-reviewer judgement.

## Answer key (adjudicated against the code)

Weight 2 marks issues that would have produced broken or unsafe code if missed.

| Key | W | Issue |
|---|---|---|
| K1 | 2 | Per-step commits leave `pub(crate)`/`pub(super)` items unused → `dead_code` fails strict Clippy |
| K2 | 2 | Create page (`data={}`, `sheet_id==0`) untested; evaluating it would show a bogus "unavailable"; D2/D6 contradict each other |
| K3 | 2 | Lifting the gate before the docs and the browser scenario allows publishing while unverified |
| K4 | 2 | Latent-bug proof is weak: test 11 passes if derived fields are omitted; the real regression is a rendered-form round trip |
| K5 | 1 | Test 4 compares `Compiled`, which has no `PartialEq` |
| K6 | 1 | Cross-thread drop of `Compiled` held in the pin needs its own small-stack test |
| K7 | 1 | Non-pin `admission::validate` callers would carry compiled ASTs needlessly |
| K8 | 2 | Q5 must not change `Evaluation`/`Code` (golden digest); `deps` is a `BTreeSet` (alphabetical, includes inputs), so derived order is needed; constraints too |
| K9 | 1 | Permit stays held after client disconnect (`spawn_blocking` not cancellable); `Retry-After: 1` is too short; 503 can follow a successful save |
| K10 | 1 | The wall-clock deadline makes route and pack tests flaky under load |
| K11 | 1 | History test compares whole pages (CSRF, editability, revision differ); the "historical pin" cannot be distinguished from the live pin |
| K12 | 1 | Derived-key submission tests miss same-count substitution and `null-<derived>` |
| K13 | 1 | `<label for>`, help and select must be suppressed for derived fields |
| K14 | 1 | "Unavailable" needs a route-level test (`input_too_large`, stored-valid sheet) |
| K15 | 1 | Fixture admission (nullable arithmetic, unresolved evidence publishable) unverified |
| K16 | 1 | Escaping test can pass for the wrong reason; needs present-escaped, attribute contexts, derived text echo, no `\|safe` |
| K17 | 1 | Browser scope incomplete, and step 6 names the wrong fixture files (`tests/support/source_fixture.rs`, `tools/smoke_release.py`) |
| K18 | 1 | `<output>` is an implicit live region, contradicting "no live region"; it also sits in the disabled fieldset |
| K19 | 1 | Derived text needs `pre-wrap` |
| K20 | 1 | Deviation from parent §7 (save evaluates proposed data) unrecorded |
| K21 | 1 | `preview` has no caller; cut it or keep it minimal and documented |
| K22 | 1 | Tests 12 and 18 are already green, so they are guards, not failing-first |
| K23 | 1 | `depart` holds up to 32 pins, so the "one alive per request" memory claim is false |
| K24 | 1 | A failed formula thread (`failed_all`) cannot be told apart from per-formula `internal_error` |
| K25 | 1 | The M1 fixture already evaluates all four characters plus the mutation; the pack test duplicates it |
| K26 | 1 | The failed-save re-render shows saved derived values beside unsaved inputs; the data source is unspecified |
| K27 | 1 | `Scalar::Text` derived values are untested |
| K28 | 1 | Test 5 passes if every code maps to one string |
| K29 | 1 | Single-formula wall time is bounded only by `MAX_COST`; the plan should cite it |
| K30 | 1 | `internal_error`/`deadline_exceeded` logging rule (no values) missing |
| K31 | 1 | Docs: the `sheet-presentation.md` htmx line concerns the native editor; code-message table needs a home; Admin Phase B coordination |
| K32 | 1 | Test 19 "no `FormulaView` built" is not observable |
| K33 | 1 | Q3 browser scope also needs keyboard `<details>` on read-only history and a native layout check |

**Added after the free-model round** (verified against the code). Models scored earlier
were not re-credited for these, except where noted. Extended maximum: 44.

| Key | W | Issue | Found by |
|---|---|---|---|
| K34 | 1 | The M1/M2 open item "`pub mod formula`, revisit at M2" (`definitions/mod.rs:5-6`) is not disposed; `preview` grows that public surface | Cat Jam, GLM, MiMo (also Space Bunny M9) |
| K35 | 1 | The pack translation spec omits the pack's top-level `encoding`, `rules_revision` and `formula_language` (`examples/tool-loop-demo/definition.json`), which the `deny_unknown_fields` envelope rejects | DeepSeek |
| K36 | 1 | The release-size gate compares against 18,210,072 B, the pre-`cel` build. M1/M2 never re-measured; the spike gave 20,288,152 B. The baseline should be the current build | GLM |
| K37 | 1 | `present(pin, data, &Evaluation)` cannot express the `InputError` → "unavailable" state; the signature or caller branch is unspecified | MiMo, DeepSeek (partial) |
| K38 | 1 | §1 "every sheet route takes the JOBS permit" is imprecise: `/private-sheets` and `/private-sheets/{id}/history` sit outside the `runtime` layer (`campaigns/web.rs:59-79`) | MiMo |
| K39 | 1 | The save-conflict path re-acquires the permit via `admit()` and, when it is busy, returns a bare 503 with no `Retry-After` and loses the draft (`campaigns/web.rs:768-769`). Longer permit holds make this likelier | MiMo |
| K40 | 2 | Hyphenated scalar field IDs stay admissible but are not declared to the engine (`formula_admission.rs:258-263`, `cel_identifier` guard), and `convert::inputs` rejects any undeclared key (`formula/convert.rs:151-153`). Passing the full canonical data therefore makes **every** evaluation of such a definition an `InputError` ("unavailable"), even when no formula references that field. M3 needs a projection of the stored values onto the declared inputs | Sol (one-off) |
| K41 | 1 | `CheckView` has no expression, so the collapsed "how this is computed" disclosure (Q4) is unspecified for constraints | Sol (one-off) |
| K42 | 1 | `preview` transition semantics are incomplete: `Violated→Error` and `Error→Violated` are undefined, so "new" and "resolved" violations are ambiguous | Sol (one-off) |
| K43 | 1 | `docs/dependency-decisions.md` (75 lines, ends with retrieval) still lacks the `cel`/`antlr4rust` record that parent plan M1 required (`derived-formulas-plan.md:411, 684`) | Sol (one-off) |
| K44 | 1 | `runtime-definition-contract.md:43-46` still gives field IDs as `[a-z][a-z0-9-]{0,63}`; M2's D2 allows `_` in field IDs | Sol (one-off) |

**Known wrong claims** (count against any model that repeats them):

| Code | Claim | Why it is wrong |
|---|---|---|
| X1 | Tokio blocking stack size unknown (as a blocker) | `#[tokio::main]`, default 2 MiB, which the M1 drop test uses |
| X2 | Make `preview` `pub(crate)` | No caller → `dead_code` → Clippy failure (harmful) |
| X3 | `store.rs:107-117` does not exist | `src/store.rs:106-117` is the DB worker |
| X4 | M2 has no review record | `docs/reviews/2026-10-15-derived-formulas-m1.md` §"M2 independent review" |
| X5 | A deadline root makes dependents report `dependency_failed` | The deadline check comes first (`formula/mod.rs:323-326`) |
| X6 | §8 lacks `--network none` | It is in `RUN` |
| X7 | Tests 13/14/16 already pass before wiring | They assert new rendering |
| X8 | D1's cost argument is wrong because the pin "recompiles anyway" | Without D1, M3 would compile twice per request (validate, then evaluate); D1 removes the second compile. The plan already says to measure before caching |

## Results (first four runs)

| | Sonnet A | Sonnet B | Muse Spark | Space Bunny |
|---|---|---|---|---|
| Wall time | 2.8 min | 2.9 min | 2.9 min | 6.4 min |
| Verdict | GO-with-amendments | GO-with-amendments | GO-with-amendments | NO-GO |
| Findings (Blocking) | 17 (2) | 25 (0) | 15 (6) | 24 (3) |
| Wrong or harmful findings | 0 found | 1 partial | 3 | 3 |
| Unique valid catches | 2 | 3 | 2 | 5 |
| Citation accuracy | high | high | medium | high |
| Severity calibration | good | slightly low | inflated | inflated |

**Wrong or harmful findings (verified):**

- **Muse F1 (Blocking):** "tokio blocking stack size is unverified". The binary uses
  `#[tokio::main]` (`src/main.rs:124`), whose blocking threads default to 2 MiB, which is
  the size the M1 drop test uses. A real follow-up test was asked for (also raised by
  Sonnet A M2), but the severity is wrong.
- **Muse F6:** make `preview` `pub(crate)`. With no caller that is `dead_code`, which is
  denied under strict Clippy. Following it would break CI.
- **Muse F12:** "`store.rs:107-117` does not exist". It looked for `src/campaigns/store.rs`;
  `src/store.rs:106-117` is the DB worker. It also called the CSS and history-template
  claims "unevidenced" without reading them; both are true.
- **Space Bunny B1 (Blocking):** "M2 has no review record". The M2 independent review is in
  `docs/reviews/2026-10-15-derived-formulas-m1.md` §"M2 independent review". The half that
  is valid: `TODO.md` is stale and still leaves M2 unchecked.
- **Space Bunny L3:** a deadline root makes dependents report `dependency_failed`. This is
  wrong: the deadline check comes first in each loop iteration (`formula/mod.rs:323-326`),
  so every later formula reports `deadline_exceeded`. Sonnet A stated this correctly.
- **Space Bunny L2 (first half):** "§8 `RUN` has no `--network none`". It does. The second
  half (the missing `source_fixture` CI command) is valid.
- **Sonnet B S2 (partial):** "tests 12, 13, 14, 16, 18 already pass". Only 12 and 18 are
  pure guards; 13, 14 and 16 assert new rendering and fail before wiring.

**Unique valid catches:**

- **Sonnet A:**
  - per-step commits leave `pub(crate)`/`pub(super)` items unused, so `clippy -D warnings`
    fails;
  - `Compiled` has no `PartialEq`, so test 4 cannot compare it.
- **Sonnet B:**
  - lifting the gate before the docs and the browser scenario leaves a window where
    publishing is allowed but unverified;
  - a GET straight after a save can get a 503 while the permit is held;
  - a rendered-form round-trip test is the real latent-bug regression.
- **Muse:**
  - test 11 passes trivially if derived fields are omitted entirely;
  - `internal_error` needs a log-without-values rule.
- **Space Bunny:**
  - a failed formula thread is indistinguishable from all formulas returning
    `internal_error` (`failed_all`, `mod.rs:304-305, 363-376`), so the planned
    "unavailable on thread failure" state cannot be produced;
  - the M1 conformance fixture already holds all four Lantern Reach characters and the
    two-outer-armor mutation (`schemas/formula-conformance-v1.json:1594…3502`);
  - the failed-save re-render shows saved derived values beside unsaved inputs;
  - the browser-fixture touch points were wrong (`tests/support/source_fixture.rs`,
    `tools/smoke_release.py`), and the existing pack fixtures carry lists;
  - `Scalar::Text` derived values are untested.
- **Shared by 2–4 reviewers (strong signal):**
  - the create page is untested;
  - the permit is held after a disconnect and `Retry-After` is too short;
  - history tests compare whole pages;
  - `<output>` is an implicit live region;
  - escaping tests can pass for the wrong reason;
  - `depart` holds up to 32 pins at once (Sonnet B, Bunny);
  - the Q5 design has to keep the golden digest (Sonnet A/B, Muse);
  - the deviation from parent §7 is unrecorded.

**Security behaviour:**

- Space Bunny tried to call `bash`. The tool was absent and the call failed closed.
- The run also showed that opencode still offered `websearch`, `task` and `skill`. The
  wrapper now denies these as well, with a regression test, and a live check confirmed that
  only `glob`/`grep`/`read`/`todowrite` remain.

## Key-based scores

The key's maximum weighted score is 38 (33 issues, five of weight 2). **Bias:** the key was
built from these first four reviews, so each of its issues was found by at least one of them.
Later models are scored against the same key and get credit for valid-new issues.

| Model | Keys hit | Weighted | Recall | Wrong / harmful | Keys only it found |
|---|---|---|---|---|---|
| Sonnet A | 20 | 23 | 61% | 0 / 0 | K1, K5, K12 |
| Sonnet B | 20 | 23 | 61% | 1 partial (X7) / 0 | K3 |
| Muse Spark | 13 | 16 | 42% | 2 (X1 severity, X3) / 1 (X2) | K29, K30 |
| Space Bunny | 18 | 19 | 50% | 3 (X4, X5, X6) / 0 | K24, K25, K26 |

Pairs (union): Sonnet A + Space Bunny reach 29 keys (weighted 32/38, 84%). Sonnet A +
Sonnet B reach 28 keys (weighted 33/38), at twice the paid cost. Cost-free pairs are added
once all free models are scored.

## Assessment per model

- **Sonnet:** the most accurate. No verified false findings, and severity is close to right.
  The two runs overlap by about half, so one run misses things. It is a paid model of the
  same vendor family as the implementer.
- **Muse Spark:** fast and concise, and it catches test-validity issues. Severity is
  inflated (six Blocking, at most two of them justified), its citations are sloppier, and
  one amendment would have broken CI. Useful as a second opinion; check everything it says.
- **Space Bunny:** the most thorough. It read the most files, found the most unique valid
  catches, and its line references are precise. It is about twice as slow, its verdict is
  harsher than the evidence supports (NO-GO driven by one false Blocking), and some claims
  rest on stale docs instead of code.

## Results (all opencode free models)

The same frozen input and prompt were used. The 9 extra models ran 3 at a time, and wall
time counts from each run's own start. Recall is the weighted score over the original 38;
"Ext" is the weighted score over the extended 44 (K34–K39 included). Files read counts
`Read` calls in the transcript. Every wrong or valid-new claim was checked against the code.

| Model | Keys hit | Weighted | Recall | Ext /44 | Wrong / harmful | Unique catches | Wall time | Files read | Blocking | Verdict |
|---|---|---|---|---|---|---|---|---|---|---|
| Sonnet A | 20 | 23 | 61% | 23 | 0 / 0 | K1, K5, K12 | 2.8 min | n/a | 2 | GO-w-a |
| Sonnet B | 20 | 23 | 61% | 23 | 1 partial / 0 | K3 | 2.9 min | n/a | 0 | GO-w-a |
| Space Bunny | 18 | 19 | 50% | 20 | 3 / 0 | K24, K25, K26 | 6.4 min | 46 | 3 | NO-GO |
| Muse Spark 1.3 | 13 | 16 | 42% | 16 | 2 / 1 | K29, K30 | 2.9 min | 28 | 6 | GO-w-a |
| MiMo v2.6 Flash | 13 | 15 | 39% | 19 | 1 / 0 | K38, K39 | 19.4 min | 33 | 1 | GO-w-a |
| DeepSeek v4.1 Flash | 12 | 14 | 37% | 16 | 1 / 0 | K35 | 5.6 min | 34 | 0 | GO-w-a |
| GLM-5.3 Flash | 8 | 10 | 26% | 12 | 1 / 0 | K36 | 9.3 min | 31 | 0 | GO-w-a |
| Cat Jam | 7 | 9 | 24% | 10 | 1 / 0 | none | 1.2 min | 21 | 5 | GO-w-a |
| LongCat 2.5 preview | 5 | 7 | 18% | 7 | 1 / 0 | none | 16.8 min | 36 | 0 | GO-w-a (implied) |
| Nemotron 3.5 Super VL | 5 | 7 | 18% | 7 | 0 / 0 | none | 5.9 min | 26 | 0 | GO-w-a (implied) |
| LL-alpha | no output | | | | | | 1.6 min | 3 | | stopped after a refused read outside the snapshot |
| Nemotron 3.5 Lightning | no output | | | | | | 14.7 min | 18 | | passed the second preflight, then idled out |
| Nemotron 3 Ultra | no output | | | | | | 2.8 min | 0 | | preflight timed out 3× |

### Paid, one-off (user's OpenAI account, 2026-10-16)

Same frozen input and prompt, run through the same read-only opencode `reviewer` agent. The
user allowed this once only. They are not candidates for the routine free set. "Ext /44"
uses the same K34–K39 extension as above. K40–K44 were found by Sol and are not credited
to any earlier model.

| Model | Keys hit | Weighted | Recall | Ext /44 | Wrong / harmful | Unique catches | Wall time | Files read | Blocking | Verdict |
|---|---|---|---|---|---|---|---|---|---|---|
| OpenAI Sol (`gpt-6.1-sol`) | 18 | 20 | 53% | 22 | 0 / 0 | **K40** (weight 2), K41–K44 | 2.9 min | 37 | 3 | NO-GO |
| OpenAI Luna (`gpt-6-luna`) | 13 | 15 | 39% | 15 | 1 (X8) / 0 | none | 1.3 min | 14 | 3 | NO-GO |

- **Sol:** the only one of the 12 reviews with output to find K40, a real defect that would make every
  formula sheet with a hyphenated input field show "unavailable". It made no wrong claims
  and cited lines precisely. It also caught the 5 s outer request timeout behind K9
  (`auth/mod.rs`). Its NO-GO is the one NO-GO in this experiment that rests on a valid
  blocker (K40). The other two Blocking findings, K23 and K9, are real but overcalled.
- **Luna:** fast but shallower: 14 files read, and its findings are mostly restatements of
  shared keys (K2, K3, K9, K11, K12, K16). It made one wrong claim (X8). Its three Blocking
  findings are overcalled. Its only notable hit is K3 (gate lift contingent on the browser
  flow), which before it only Sonnet B had found.
- **With Sonnet (union, /38):**
  - Sonnet A + Sol: 29 (76%).
  - Sonnet A + Sol + Space Bunny: 32, the same as Sonnet A + Space Bunny. Sol's value is
    K40–K44, which lie outside the original key.
  - Sonnet A + Sol + Luna: 32.

**Wrong claims in the new round (verified):**

- **Cat Jam B1 (Blocking):** presents D1's own design (forward `Compiled` into `Admitted`)
  as a missing requirement.
- **DeepSeek A14:** says `present` duplicates `FieldView::valid_boolean`/`valid_option`.
  Those validate submitted input strings; they do not format derived values.
- **GLM item 10:** says lowercasing rule IDs in the pack translation is a no-op. It is
  required: rule IDs must match `registry::valid_id`, which allows only lowercase, and the
  pack test builds the envelope.
- **LongCat F6:** says the fixture should "bypass the gate via `approve_definition`".
  `approve_definition` is the path that enforces the gate.
- **MiMo F12:** misquotes the plan's failed-save label, and claims checks are computed from
  unsaved data. The plan says "computed from saved revision N".

**Behaviour seen:**

- Cat Jam and Nemotron Super VL tried to call `bash`; it is absent, so the calls failed
  closed.
- LL-alpha tried to read `~/.claude/CLAUDE.md`. opencode refused, and the wrapper now
  denies `external_directory` outright.
- MiMo printed its own context-compaction notes into the review before the verdict.
- Nemotron 3 Ultra and Lightning were too slow for the 45 s preflight or the idle timeout.

**Coverage limits:** all free models together reach 31/38. None of them found K1
(per-step commits fail strict Clippy), K3 (lift the gate last), K7, K12 or K33. K1 and K3
are weight-2 issues found only by Sonnet.

**Best cost-free combinations** (union, weighted /38, extended /44):

- **Pairs:**
  - Muse + Space Bunny: 27/38 (71%), 28/44.
  - Space Bunny + DeepSeek: 26/38, 29/44.
  - Space Bunny + MiMo: 25/38, 29/44.
- **Trios:**
  - Muse + Space Bunny + DeepSeek: **31/38 (82%)**, 34/44.
  - Muse + Space Bunny + MiMo: 30/38, 34/44.
  - Either trio already reaches the ceiling of all free models combined (31/38).
- **With Sonnet:**
  - Sonnet A + Space Bunny: 32/38, 33/44.
  - Sonnet A + Space Bunny + MiMo: 32/38, 36/44.

## Assessment of the new models

- **DeepSeek v4.1 Flash:** the best new free reviewer. It checked every §1 citation, set
  severity sensibly (0 Blocking), raised one wrong low-severity point, and found
  a concrete fixture blocker (approval needs at least one mapped field). Moderate speed.
- **MiMo v2.6 Flash:** finds real, subtle issues that other models miss (the conflict-path
  bare 503, the route-layer imprecision, the `present` signature gap). It is very slow
  (19 min), its output is noisy (it leaked its working notes), and it misquoted the plan
  once.
- **GLM-5.3 Flash:** precise but narrow. Its one unique catch is useful (a stale release
  baseline), and it made one wrong claim.
- **Cat Jam:** very fast, but more than half of its 18 findings restate the plan or ask
  for tests the plan already has. Severity is inflated (5 Blocking). Low value.
- **LongCat 2.5 preview:** slow (17 min), and its 6 findings all duplicate others. It
  dismisses the permit-after-disconnect issue as "not a defect".
- **Nemotron 3.5 Super VL:** no wrong claims, but shallow. Nemotron 3 Ultra and Lightning
  were unusable within the timeouts.
- **LL-alpha:** unusable (no review, and it attempted an out-of-scope read).

## Recommendation (updated after all free models)

- **Keep one Sonnet run in every review.** No free model found the two weight-2
  process/CI issues (K1, K3), and Sonnet had the fewest wrong claims.
- **Free reviewers to run in parallel with it:** Space Bunny and DeepSeek v4.1 Flash. They
  are the most thorough free model and the most accurate new one. Add MiMo when time
  allows, because its unique catches are real, but it is slow.
- **Muse:** keep it optional. It adds coverage alongside Space Bunny (the best free pair
  overall), but it inflates severity and once made a harmful suggestion.
- **Drop:** Cat Jam, LongCat, Nemotron (all three) and LL-alpha.
- **Always** check Blocking findings against the code before acting. Free-model verdicts
  are not reliable on their own: Space Bunny's NO-GO and Cat Jam's 5 Blocking were both
  driven by wrong claims.
- **Caveat:** this is one plan and one run per model. Treat it as a first data point and
  re-check on the next review (for example, the M3 implementation review).

The first round's new valid findings are folded into the M3 plan §10 (items 17–30), and K34 is covered by item 29. K35–K39 are in §10c (items 31–36). **K40–K44 (Sol) are not yet in the plan.** They need a §10d amendment before M3 starts, and K40 is weight 2.

- **Paid one-off result:** Sol was the strongest single reviewer after Sonnet. It found a
  blocker that all 11 other reviews with output missed. This does not change the routine free
  recommendation above, but it shows that even the best free trio misses real defects. A
  stronger paid model is worth adding for milestone gates (plan, and implementation before
  merge) if the user wants it again.

# Experiment 2: harness v2 results review (2026-10-16)

- **Input:** the results and failure-analysis sections, the harness, the pack, and both
  ledgers (about 10 MB of JSONL) in the sanitized snapshot.
- **Same prompt for all three.** The opencode reviewers cannot execute code; Sonnet could
  run stdlib Python inside the snapshot.
- **No answer key.** Findings were adjudicated directly; see
  [review record](reviews/2026-10-16-tool-loop-v2-results.md).

| | Sonnet B | Muse Spark | Space Bunny |
|---|---|---|---|
| Wall time | 3.0 min | 2.8 min (12:34:35–12:37:22) | 4.8 min (12:34:35–12:39:21) |
| Verdict | SOUND-with-corrections | SOUND-with-corrections | SOUND-with-corrections |
| Findings (Blocking) | 11 (0) | 13 (5) | 17 (3) |
| Verified wrong | 0 | 2 | 1 (bug framing) |
| Unique valid catches | seed-agreement error (needed computation), `length` count via ledger, Dara confound in 4B | `wrong_proposal` summary rate, Dara-stratified reporting | at-risk correct rate, missing limitations, `MAX_TURNS` hard cap, ask_fact false-supported bookkeeping |
| Recomputed numbers | yes (Python) | no (grep failed on long JSONL lines) | yes, by reading `run_summary` and individual ledger lines |

**Observation:** on data-heavy reviews, a reviewer that can execute code has a structural
advantage. The opencode `reviewer` agent has no shell by design, and Muse's grep failed on
JSONL records longer than 64 KiB. One option is to give opencode reviewers pre-computed
per-episode CSV extracts.

# Experiment 3: harness v3 plan review (2026-10-16)

- **Setup:** six reviewers in parallel on one sanitized snapshot, with an identical prompt.
  Sol and Luna are the user's own OpenAI models, used **one-off** at the user's request.
- **Adjudication:** a main-session fork checked each finding against the files; see the
  [review record](reviews/2026-10-16-tool-loop-v3-plan.md). There is no answer key, so
  "valid" is the adjudicated count.

| Reviewer | Wall time | Findings (Blocking) | Valid | Wrong / harmful | Unique valid catches | Citations | Severity | Verdict |
|---|---|---|---|---|---|---|---|---|
| Sonnet 5.5 | 1.9 min | 17 (3) | 16 | 1 minor citation / 0 | mechanical gold oracle; v2-flags control; thinking-arm interpretation; host raw vs echoed arguments; ≥8 non-Dara passive | high | good | GO-with-amendments |
| Space Bunny | 3.9 min | 28 (7) | 25 | 3 / **2 harmful** (hedge-guard change; English-only bar) | prompt/tools hashes missing from the merge guard; 35B cold-read vs the 900 s health wait; completeness gate; pin every model-visible string | high | inflated | NO-GO |
| DeepSeek v4.1 Flash | 3.4 min | 15 (1) | 15 | 0 / 0 | arm-name collision; scheduling the v3 results review | high | good | GO-with-amendments |
| Muse Spark 1.3 | 1.7 min | 15 (8) | 13 | 2 / 0 | user-as-adjudicator contamination | medium | inflated | NO-GO |
| GPT-6.1 Sol | 2.4 min | 18 (7) | 18 | 0 / 0 | held-out visible before selection; "clamped" is wrong (it is rejected); `CHECK_ONLY` stops before launch; lenient must keep other reasons; narrower refuse wording; H4 test via the CLI | high | somewhat inflated | NO-GO |
| GPT-6 Luna | 1.6 min | 16 (6) | 15 | 1 partial / 0 | reserve cases (the minimums sum to exactly 150) | medium | inflated | NO-GO |

**Observations across experiments 1–3**

- **Consistently accurate:** Sonnet and Sol. Sol had zero wrong findings in two experiments
  and found the most serious unique bug in experiment 1 (K40).
- **Best free models:** DeepSeek is accurate and calibrated but narrower. Space Bunny is the
  broadest, but repeatedly over-severe, and it is the only reviewer whose suggestions were
  harmful (in two of its three experiments).
- **Muse and Luna:** fast, with inflated severity and few unique catches.
- **Verdicts are unreliable on their own:** four NO-GOs against two GO-with-amendments for
  the same plan, all of which turned out fixable in the plan text. Always adjudicate.
