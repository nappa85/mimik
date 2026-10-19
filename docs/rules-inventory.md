# Rules inventory (for owner review and pruning)

2026-10-19. Read-only survey of `AGENTS.md`, `TODO.md`, `README.md`, `docs/*.md`, the
"accepted decisions" in `docs/reviews/`, and the assistant's memory files. Nothing here has been
changed in code. Its purpose is to let the owner see which rules they actually made and which ones
the assistant, plans or reviewers added on their own.

**Origin codes.**
- **U**: explicit user decision (quoted or dated where the source says so).
- **D**: a design decision by the assistant, a plan or a reviewer, not by the user.
- **S**: a security or data-protection rule.
- **P**: a process rule (reviews, tests, commits, schedule).
- **unclear**: the source doesn't say who decided. Most of `AGENTS.md` dates from the first
  commit (`363ce3d`, 2026-10-15) without per-rule attribution; only the lines tagged "user
  decision" are certain.

**Flags.**
- ⚠ = a D rule that looks like a product/UX choice the user may never have made consciously.
- ⚡ = conflicts with the newest direction (2026-10-19): "help as much as we can, don't force
  anything; improve paper and pencil, don't replace it". That means free text in fields,
  arithmetic like `84 -15`, warnings with revert and suggestions instead of refusals,
  typeahead from catalogues, interactive disambiguation, JavaScript for interactivity, and
  possibly LLM interpretation on CPU.

## Most likely to revisit

1. ⚠⚡ **"No JavaScript on sheets"** (`catalogue-plan.md:102`, `:311`; step-4 plan `:12`). This
   is D. The user said the opposite on 2026-10-15: "JavaScript may provide browser interaction"
   (`discovery.md:122`). What they rejected was *template* JavaScript for derived values only
   (`discovery.md:144-147`). The "rule" is a side effect: in auth mode the legacy htmx asset
   answers 403 (`src/auth/tests.rs:1037-1042`). Other pages already ship scripts (README:69,
   material pages).
2. ⚠⚡ **Integer fields accept only exact decimal i64** (`runtime-definition-contract.md:283-291`,
   `src/campaigns/values.rs:164-173`). `84 -15`, `+3`, ` 7` and `07` are all refused with 422.
3. ⚠⚡ **Structural invalidity refuses the save with 422** (`runtime-definition-contract.md:304-307`).
   This covers missing keys, bad grammar, required fields left empty
   (`:296-300`), and a catalogue key that isn't in the catalogue (`catalogue-plan.md:298-301`).
   Advisory formula checks already never block a save; structural checks do.
4. ⚡ **"An item not in the catalogue is invalid"** (Q4, `catalogue-plan.md:29`). This one is U
   (2026-10-18), but it conflicts with "free text in fields" and "don't force". Worth asking
   whether Q4 becomes "warn and offer the catalogue match".
5. ⚠⚡ **No datalist and no per-row select; picking is a server round trip**
   (`catalogue-plan.md:311-330`, step-4 plan `:319-330`). This is D. It rules out typeahead and
   interactive disambiguation.
6. ⚠⚡ **No live recompute while typing; full-page POST/redirect only on runtime sheets**
   (`sheet-presentation.md:15-22`). D. It follows from the no-JS assumption.
7. ✅ **403 vs 404 — decided by the user 2026-10-19**: 404 for a missing entry or type
   inside a linked catalogue, a uniform 403 page *with a meaningful message* for every other
   refusal, 400 with a message for malformed addresses (branch `catalogue-decisions`, plan §19).
8. ⚠ **Catalogue administration is CLI-only, with no withdrawal command**
   (`catalogue-plan.md:106`, `:228`). D. The user already granted Owner *browser* authority for
   rulesets and definitions (`discovery.md:159-161`), so CLI-only catalogues are inconsistent
   with that.
9. ⚠⚡ **Constraint severity `error` "blocks save"** (`definition-language-draft.md:80`). D,
   and stale: the user decided "violations warn, never block" (`definition-language-draft.md:9`).
   The review accepted it (`reviews/2026-10-18-definition-language-draft.md:33`), but the
   block table still says it.
10. ⚠⚡ **Model role limited to authoring definitions, "never on the correctness path"; no LLM
    in the catalogue plan** (`reviews/2026-10-18-definition-language-draft.md:43`,
    `catalogue-plan.md:36-48`). The direction is U (2026-10-18), but the strict wording is D.
    The newest direction allows LLM interpretation on CPU, and AGENTS.md:8 (U, 2026-10-19) now
    allows small CPU models during the day.
11. ⚠ **Product caps**: 128 campaigns total, 32 per creator; 32 sheets per custodian and
    campaign; 2,048 private sheets total (`campaign-workspace.md:109-111`,
    `src/campaigns/domain.rs:145`, `sheets.rs:291`). D. Also the catalogue cap of 6,144 entries
    (D, calibrated; the user deferred raising it as "early optimization", `poc-first.md`).
12. ⚠ **Dice rolling is out of scope (notation only)** (`definition-language-draft.md:87`). D.
    It was an open question for the user (`discovery.md:102`).

## Product / UX

| Rule | Origin | Where | Constrains | Flag |
|---|---|---|---|---|
| Standalone, open-source, sheet-first companion for the physical table; not a VTT | U | `discovery.md:74,94`; `architecture.md:7` | scope | |
| Rust app with an htmx web UI | U | `discovery.md:67`; `architecture.md:3` | stack | |
| JavaScript may provide browser interaction; no new scripting language or rules DSL | U (2026-10-15) | `discovery.md:122-123`; `roadmap.md:10-11`; `TODO.md:175` | design | |
| Template JavaScript for **derived values** rejected (browser/server round-trip); use declarative formulas in Rust | U (2026-10-15) | `discovery.md:144-150`; `TODO.md:288` | CEL formulas (`src/definitions/formula`) | |
| Scoped DSL exception: CEL via the `cel` crate for derived fields only | U (2026-10-15) | `discovery.md:162-170` | `formula_profile: 1` | |
| "No application JavaScript framework" (htmx is JS, so not "JS-free") | D | `architecture.md:18` | stack | |
| **No JavaScript on private sheet pages; htmx denied in auth mode** | D | `catalogue-plan.md:102`; `reviews/2026-10-18-catalogue-plan.md:71`; `src/auth/tests.rs:1037-1042` | all runtime-sheet interaction | ⚠⚡ |
| Pick and view catalogue entries "without JavaScript", via server ops (`browse`, `pick`) | D | `catalogue-plan.md:311-330`; `catalogue-step4-plan.md:11-14,612` | step 4 design | ⚠⚡ |
| No datalist, no per-row select (size, budget, accessibility) | D (reviews) | `catalogue-plan.md:329-330` | typeahead impossible | ⚠⚡ |
| Runtime sheets: full-page POST/redirect/GET; no live recompute while typing | D | `sheet-presentation.md:15` | formulas shown only after save | ⚠⚡ |
| Derived values are read-only text with a "Computed" label, never form controls | D | `sheet-presentation.md:15` | no override of derived values | ⚠ |
| List rows: Add/Remove/Move each take one round trip; no script | D | `sheet-presentation.md:16-22`; `derived-formulas-m4-plan.md:609` | M4 list UX | ⚠⚡ |
| Booleans are unknown/true/false selects, not checkboxes | D | `runtime-definition-contract.md:290-291` | form controls | ⚠ |
| Recreate the official sheet structure in adaptive HTML/CSS with long-text growth and mobile reflow | U | `discovery.md:86,97`; `sheet-presentation.md:5`; `AGENTS.md:14` | presentation | |
| Don't call the current generic editor an official sheet | unclear (AGENTS.md, undated) | `AGENTS.md:14`; `roadmap.md:56` | wording | |
| Print/export is a separate, later feature | U (priorities still open) | `discovery.md:97`; `TODO.md:205` | | |
| **Assist players, don't police them**; no automatic GM alert feeds or monitoring dashboards; GM review is allowed | U (2026-10-18) | `definition-language-draft.md:17-28` | design | |
| Rule violations warn and never block; log, revert, suggest a correction | U (2026-10-18) | `definition-language-draft.md:9-10`; `TODO.md:9` | not yet implemented (revert is out of scope in `catalogue-plan.md:42`) | |
| Advancement is context-driven: an out-of-context edit warns; the level-up trigger opens a guided "happy path" | U (2026-10-18) | `definition-language-draft.md:11-15` | future flows | |
| Players edit sheets; the app checks legality, does arithmetic, guides advancement, offers legal choices, shows descriptions on click; chat last | U (2026-10-18) | `TODO.md:7`; `definition-language-draft.md:32-39` | roadmap | |
| Constraint severity `error` blocks the save | D (draft) | `definition-language-draft.md:80` | stale versus the U decision above | ⚠⚡ |
| Dice rolling outside scope; notation only; the player enters the result | D | `definition-language-draft.md:74,87,172` | value types | ⚠ |
| Manual page images not shown to players; citation only ("later decision") | U (Q1, 2026-10-18) | `catalogue-plan.md:26` | entry pages | |
| Unreviewed catalogue entries shown with an "unchecked" badge | U (Q2) | `catalogue-plan.md:27` | badges | |
| Owner-written (homebrew) entries allowed, badged | U (Q3) | `catalogue-plan.md:28` | | |
| No switch to restrict catalogue text from players | U (Q5) | `catalogue-plan.md:30` | | |
| Catalogue browse only through a sheet; no campaign-level browse | D | `catalogue-plan.md:43,332` | routes | ⚠ |
| Retired entries stay listed with a "Retired" badge (U, 2026-10-19); **new picks of them are refused** (D) | U + D | `reviews/2026-10-19-catalogue-step3.md:62`; `catalogue-plan.md:628-629,786` | 422 on pick or save | ⚠⚡ |
| 404 for a missing entry/type in a linked catalogue; uniform meaningful 403 otherwise; meaningful 400 | U (2026-10-19) | `reviews/2026-10-19-catalogue-step3.md:59-60`; `TODO.md:26`; `catalogue-step4-plan.md:40,319-330` | status codes | |
| Stale wiring keeps a sheet's catalogues readable ("out of date doesn't mean deleted") | U (2026-10-19) | `reviews/2026-10-19-catalogue-step3.md:61`; `TODO.md:26` | | |
| Stale wiring blocks **new character creation** from that definition | D | `runtime-definition-contract.md:229-231`; `campaign-workspace.md:93` | creation | ⚠ |
| History diff names what changed, never old or new values | D | `catalogue-plan.md:104`; `runtime-definition-contract.md:365` | history UI | ⚠ |
| Catalogue entry description cut at 20,000 characters | D | `TODO.md:26` | entry page | |
| Owner account is administration-only in normal mode; it cannot play | U (2026-10-13) | `discovery.md:28-30`; `collaboration-and-history.md:45-49` | auth | |
| `--dev-owner-play` development override | U | `discovery.md:30`; `campaign-workspace.md:25-29` | | |
| Owner may register rulesets and publish/withdraw definitions from the browser | U (2026-10-15) | `discovery.md:159-161` | `/admin/rulesets` | |
| **Catalogue administration is CLI-only, no withdrawal** | D | `catalogue-plan.md:106,228` | no catalogue admin UI | ⚠ |
| Game-agnostic core: no per-game Rust, game concepts are data | U (2026-10-15, 2026-10-18) | `discovery.md:128-129`; `definition-language-draft.md:41-44,86`; `model-assisted-games.md:5` | architecture | |
| Native MÖRK BORG code is a fixture only; no new native per-game modules | U | `discovery.md:63`; `model-assisted-games.md:36` | | |
| Ship plain list editing in M4; `editable` flag and item catalogue deferred | U | `TODO.md:35` | | |
| Testing needs a self-authored synthetic demo pack; no empty UI | U | `TODO.md:176` | demo | |

## Data and validation

| Rule | Origin | Where | Constrains | Flag |
|---|---|---|---|---|
| Formula results are advisory: never stored, and checks never block a save | D (consistent with U warn-never-block) | `runtime-definition-contract.md:41-45`; `campaign-workspace.md:47-48` | `src/campaigns/formulas.rs` | |
| No supplied minima/maxima, prerequisites or model-generated restrictions enforced | D | `runtime-definition-contract.md:135-137` | | |
| **Integer = exact decimal i64 token only** (no `+`, spaces, leading zeros, expressions) | D | `runtime-definition-contract.md:283-291`; `src/campaigns/values.rs:164-173` | `84 -15` is refused | ⚠⚡ |
| Full-replacement saves; every declared key required on every save | D | `runtime-definition-contract.md:274-278` | | |
| Creation needs explicit full values; no manufactured defaults | D | `runtime-definition-contract.md:296-299` | create form | ⚠⚡ |
| Required non-nullable fields need a value before saving | D | `runtime-definition-contract.md:299-300` | 422 | ⚠⚡ |
| Structural invalidity gives 422 with the draft; a stale write gives 409 | D | `runtime-definition-contract.md:304-307` | save path | ⚠⚡ |
| Name trimmed, 1–100 characters; text 8,000, notes 20,000 | D | `runtime-definition-contract.md:300`; `campaign-workspace.md:104-105` | | |
| A sheet row naming an item not in the catalogue is **invalid** | U (Q4, 2026-10-18) | `catalogue-plan.md:29` | membership check, 422 (`catalogue-plan.md:298-301`) | ⚡ |
| Catalogues are append-only: keys and types never disappear; publication refuses drops or retyping | D (derived from U Q3+Q4) | `catalogue-plan.md:32-35,160-173` | `migrations/019` | ⚠ |
| Owner text: LF only (CRLF refused), Cf/Zl/Zp refused in names, blank-looking text refused | D (author decisions) | `catalogue-publication.md:93-112`; `reviews/2026-10-19-catalogue-step1.md:30` | import CLI | ⚠ |
| Text property cap of 1,000 scalars; at least one entry type | D (author) | `reviews/2026-10-19-catalogue-step1.md:34-38` | | |
| `catalogues: []` refused | D (author) | `reviews/2026-10-19-catalogue-step3.md:64` | | |
| After the merge, any admission change needs a new profile id | D | `reviews/2026-10-19-catalogue-step2.md:72-74` | | |
| Catalogue caps: 6,144 entries and unit references; owner text 1 MiB | D (calibrated) | `TODO.md:21-22` | | ⚠ (user said "POC first", `poc-first.md`) |
| Campaign and sheet capacity caps (128/32 campaigns, 32/2,048 sheets, 64 members) | D | `campaign-workspace.md:109-111`; `src/campaigns/domain.rs:145`; `src/campaigns/sheets.rs:291` | | ⚠ |
| Definition 256 KiB, 1,024 evidence references, snapshot 1 MiB | D | `catalogue-plan.md:66-68`; `runtime-definition-contract.md:153` | | |
| Every sheet belongs to a campaign bound to an exact game/edition; cross-edition refused | U (2026-10-12) | `discovery.md:43-46`; `collaboration-and-history.md:24-27` | | |
| No automatic assignment of legacy/prototype sheets to campaigns | unclear | `discovery.md:45-46`; `runtime-definition-contract.md:27-28` | | |
| Prototype records are disposable; wipe rather than weaken constraints | U | `discovery.md:34-36`; `runtime-definition-contract.md:30` | | |
| Cross-scope (campaign, scenario) reads forbidden in derived values for now | D (review) | `reviews/2026-10-18-definition-language-draft.md:37` | future | |
| Extracted Markdown is never authoritative; check it against the PDFs | U + AGENTS | `discovery.md:78`; `source-quality.md:5`; `AGENTS.md:16` | source review | |
| Only reviewed passages support verified explanations | D | `source-quality.md:29`; README (reviewed-only lookup) | lookup | (Q2 already loosens this for catalogues) |
| Never guess a missing action cost from text | D | `source-quality.md:27` | extraction | |
| Use deterministic, source-backed rules for calculations and state changes | unclear (AGENTS.md) | `AGENTS.md:15`; `architecture.md:11` | | |

## Security and privacy

| Rule | Origin | Where | Constrains | Flag |
|---|---|---|---|---|
| Never print or read back `.env` or credentials; test with dummy secrets | S (AGENTS) | `AGENTS.md:17`; `discovery.md:85` | | |
| Manuals, corpora, rendered pages and databases stay local, outside the repo and releases | S | `AGENTS.md:17`; `local-setup.md:25`; `catalogue-plan.md:30` | | |
| Sheets and history private to the custodian and campaign Masters; Owner has no implicit access | U (2026-10-12/13) | `collaboration-and-history.md:17-18,59-61` | auth | |
| Invite-only enrollment; joining is a request | U | `collaboration-and-history.md:10,14-16` | | |
| Passwords: Owner ≥ 15, players ≥ 12, no required categories | U | `discovery.md:31-32` | | |
| Private LAN only; TLS optional, Owner-configured | U | `collaboration-and-history.md:11-13` | | |
| `--no-auth` prototype is loopback-only and isolated from private tables | D/S | `deployment.md:37-39`; `campaign-workspace.md:74-75` | | |
| Departure freezes sheets; rejoin never thaws; a Master approves unfreezing | U (2026-10-13/15) | `collaboration-and-history.md:62-71,115-118`; `campaign-workspace.md:79` | | |
| Lifecycle: Owner recovery by CLI; re-enable keeps memberships; transfer within a campaign with acceptance; Owner restores; purge removes everything, including the audit | U (2026-10-15) | `collaboration-and-history.md:108-128` | not yet implemented | |
| Not-found answered as 403 on sheet routes | D/S | `catalogue-plan.md:98-99` (`web.rs:123-125`) | | ⚠ (see 403 vs 404) |
| Anti-framing CSP (`frame-ancestors 'none'`), no-store; no script-src restriction | S | `docs/http-auth.md:63`; `src/auth/mod.rs:436` | does **not** forbid JS | |
| Owner-uploaded templates are data; their scripts are never executed | S/D | `experience-validation-plan.md:76-79`; `model-assisted-games.md:22` | | |
| Imported documents are data, never model instructions | S | `architecture.md:68` | | |
| Integers never pass through JS `Number` | D/S | `runtime-definition-contract.md:287`; `definition-drafts.md:23` | relevant if JS is added | |
| Picker query never stored | D | `catalogue-plan.md:560` | | |
| Held-out ledgers read only through aggregate `--summarize`; never open the frozen case file | P/S (main session, 2026-10-17) | `TODO.md:28`; `tool-loop-heldout-v3-freeze.md` | benchmarks | |
| Reviewers: only repo source and docs on the free tier; never the opencode `plan` agent; never snapshot `.env`, `data/`, `target/` or manuals | S | `review-routing.md` (memory) | `tools/external_review.py` | |
| Install new tools and runtimes in containers, not on the host | S/P (AGENTS) | `AGENTS.md:19` | | |

## AI / model use

| Rule | Origin | Where | Constrains | Flag |
|---|---|---|---|---|
| Model output can't establish legality or automatically alter sheets; changes need explicit user confirmation | U (tool-loop decision) + AGENTS | `AGENTS.md:15`; `discovery.md:140-143`; `architecture.md:13` | | compatible with "suggest" |
| Tool-calling loop (`search_rules`, `read_fields`, `propose_change`, `ask_user`); Rust fills old values and the diff | U (2026-10-15) | `discovery.md:140-143` | harness | |
| Runtime chat model is text-only; vision only for offline ingestion | U (2026-10-15) | `discovery.md:122,152-153` | | |
| Local inference with RAG is required for ordinary use; the API key is only for OCR/extraction review; don't use the work Anthropic account | U | `discovery.md:125-127`; `roadmap.md:12-14` | | |
| Accuracy bar: ≥ 90% correct, ≤ 2% false-supported on ≥ 100 held-out cases, gating model-dependent product work | U (2026-10-15) | `discovery.md:171-179` | any LLM feature | ⚡ (does it gate *advisory* interpretation?) |
| The LLM's main job is authoring definitions with owner review, not judging edits at play time | U (2026-10-18) | `TODO.md:7`; `definition-language-draft.md:43-46` | | ⚡ (newest: interpretation on CPU) |
| "The model is never on the correctness path" | D (review) | `reviews/2026-10-18-definition-language-draft.md:43` | | ⚠ |
| No LLM anywhere in the catalogue plan | D (scope) | `catalogue-plan.md:36-48`; `catalogue-step4-plan.md:30` | | ⚡ |
| Text rules: the optional play-time LLM may raise warnings, never errors | D | `definition-language-draft.md:84` | | |
| Rules queries never spend resources or alter sheets | D | `model-assisted-games.md:25`; `experience-validation-plan.md:69` | | |
| Automated or model review never auto-approves evidence | U + AGENTS | `discovery.md:17`; `AGENTS.md:16`; `roadmap.md:84` | | |
| Provider support through an established Rust abstraction (genai/Rig), not hand-written protocols | U | `discovery.md:85`; `AGENTS.md:13` | | |
| Small CPU models (≤ ~4B, quantized) allowed on weekdays, 2 cores | U (2026-10-19) | `AGENTS.md:8` | | enables CPU interpretation |

## Process and machine

| Rule | Origin | Where | Constrains | Flag |
|---|---|---|---|---|
| Start each session with `TODO.md` and keep it updated | unclear (AGENTS, undated) | `AGENTS.md:3` | | |
| Commit locally without asking; pushing needs approval | U (2026-10-16) | `AGENTS.md:4`; `local-commits.md` | | |
| Weekdays 09:00–18:00: no heavy work; builds and tests capped at 2 cores; weekends unrestricted | U (2026-10-16) | `AGENTS.md:5-7`; `machine-schedule.md` | | |
| Downloads allowed after licence checks, stored in `/media/data/mimik`, pinned and hashed | U (2026-10-16) | `AGENTS.md:9` | | |
| Unattended runs enforce the window themselves and never wait for input | U (feedback 2026-10-16) | `AGENTS.md:10`; `unattended-runs.md` | | |
| GPU benchmarks blocked until the GPU is re-pasted; queue refuses GPU work unless the fan is manual and high | U (CPU-only approved) / D (fan guard) | `TODO.md:48-58,107-108`; `machine-schedule.md:23-26` | queue | |
| Prefer maintained crates; record selection decisions | U ("global rule") | `discovery.md:84`; `AGENTS.md:11` | | |
| Rust ships; Python only for prototypes | U | `discovery.md:76`; `AGENTS.md:12` | | |
| Independent adversarial review of every plan and material change set | unclear (AGENTS, undated) | `AGENTS.md:23-30` | all work | |
| Reviewers: Sol + Sonnet main; third chain Space Bunny → DeepSeek → Muse | U (2026-10-16) | `AGENTS.md:26-29`; `review-routing.md` | | |
| Prototype tier: a lighter process for disposable experiments | U (2026-10-15) | `AGENTS.md:32-36` | | |
| Failing-first permanent regression test for every reproducible issue | unclear (AGENTS) | `AGENTS.md:40` | | |
| CI fail-closed; Clippy `all` + `pedantic` denied | unclear (AGENTS) | `AGENTS.md:43-44` | | |
| Never report unrun checks as passed | unclear (AGENTS) | `AGENTS.md:18` | | |
| Ask open decisions freely ("only active blockers" was temporary) | U (2026-10-15) | `question-policy.md` | | |
| Working end-to-end POC before perf or cap optimization | U (2026-10-19) | `poc-first.md` | | |
| Record decisions in `discovery.md`, separating user decisions from proposals | D | `TODO.md:148` | | (this inventory shows that has slipped: catalogue and author decisions live only in plans and reviews) |

## Notes

- **"Strict validation" as a whole** has no user origin. The user's only explicit validation
  policy is warn-never-block (2026-10-18). Before that it was an open question
  (`discovery.md:101`). Strictness lives in D rules: the integer grammar, full-replacement
  saves, required fields, the 422 refusals, and the owner-text character rules.
- **No user decision exists for** live recompute, typeahead, CLI-only
  catalogues, the capacity caps, dice, or retired-entry refusal.
- **Superseded but still written:**
  - "Admin work paused" (`discovery.md:118`, `roadmap.md:17-18`), superseded by "admin
    continues" (`discovery.md:158`);
  - "No rules DSL required for the trial" (`model-assisted-games.md:24`), superseded by the
    CEL exception and the definition-language draft;
  - `error` blocks save (`definition-language-draft.md:80`).
