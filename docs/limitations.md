# Known limitations and follow-ups

One entry each: `**ID** [area]`, the limit, and what would close it. When an entry is fixed, delete it; git history keeps it.
Decision IDs refer to `docs/decisions.md`. Entries marked "(later)" are accepted for now and wait for a product step.

## Assisted sheet entry

- **L1** [notes] Change notes are unsealed (D-T): the revision actor's session or any database writer can add notes to the live revision, and a malformed latest note hides a real scribble. Notes are help, not evidence; sealing and read-time verification are required before release.
- **L2** [entry] Without JavaScript, text typed in another field and not entered is lost when a unit save reloads the page; switch and Undo carry no draft. With the sheet module unit saves do not reload, a left field saves first and page posts wait for those saves (ASSIST-24); a draft refused with JavaScript (ASSIST-23) is still lost when a page post reloads.
- **L3** [entry] Unit conflicts compare `base` and live snapshots, so A → B → A goes undetected. Deliberate.
- **L-U1** [notes] A save that would write more than 256 notes (unreadable or erased cells included) is refused with 422.
- **L-U2** [storage] No rollback path: an older binary refuses a sheet holding `null` in a non-nullable integer.
- **L-U3** [formulas] `has(item.p)` is true for a present but unknown (`null`) property; it does not prove the value is known.
- **L-U4** [formulas] A definition cannot give an unknown integer a fallback; `== null` is none (D-Z). Closed by letting definitions choose a fallback for unknown values.
- **ASSIST-L2** [pages] Page-size headroom is thin: tightest worst case 18,447,369 of 18,948,096 bytes (2.6%: a list of 256 rows × 32 integers with 64-character IDs, labels of 100 `"`, a kept 200-`"` scribble on every cell, each cell its own form); 256 wide scalars with long scribbles 3.1%; among re-renders, a new row kept read-only from a full list of kept scribbles 23,282,470 of 23,748,096 (2.0%, bound plus echo) and an orphaned cell's draft 2.5%. Rerun the worst-case fixtures in `src/campaigns/list_tests.rs` whenever per-cell markup grows.
- **ASSIST-L11** [pages] Each list's "Row removed · Undo" reads that list's row notes on every page view (`notes::latest_rows`, one index range per list), so its cost grows with the list's removal history. Closed by an index or table keyed by list if it shows up in profiles.
- **ASSIST-L12** [entry] Quick-add counts are digits only: a count written as a word ("three torches", "tre torce") stays in the name with a count of 1; there is no Undo of the added row (POC cut: Undo of creation; Remove removes it).
- **ASSIST-L13** [entry] A refused number or text cell entry holding a line break is echoed in a single-line `<input>`, which drops the break, so a retry posts altered text (the entry was refused for that character anyway). The quick-add line and the busy page use a textarea; the cells would need the same switch.
- **ASSIST-L15** [script] After a save with JavaScript the rest of the page is not refreshed: the header's snapshot revision, the change summary, values and Undo of other units changed in another tab, and the page's shared row-action, switch and Undo forms keep the revision the page was loaded at: edit a cell with JavaScript, then press Remove on its row, and the first press answers 409 (fail-closed; the re-rendered page, at the live revision, removes it on the second press). A reload shows everything.
- **ASSIST-L16** [script] When a save makes the engine refuse or accept the saved values (the derived and checks sections change shape), the module asks for a reload instead of redrawing them; the validity flag is still updated.
- **ASSIST-L17** [script] The module has no JavaScript linter or unit tests in CI: a Rust token lint (no HTML sinks, no styling, attribute names from an allowlist, no code from strings) and the Chromium runs cover it. Answers that are not JSON (busy 503, 403, network failure) show a generic alert under the cell, without the busy page's retry form.
- **ASSIST-L18** [script] A link followed right after editing a field can abort that field's save on leaving (the navigation cancels the request; the server may or may not have written it), and Save clicked after the leave-save already answered sends a second, harmless "No change" request. A page whose formula sections change shape is not exercised in Chromium (ASSIST-L16 is covered by reading only).
- **ASSIST-L19** [script] After a save aborted at 30 s or lost to a network failure the write may still have landed; a retry then answers 409 whose saved value equals the draft (the alert shows it; Enter again is a harmless overwrite).
- **ASSIST-L33** [script] A Use abandoned at 30 s (like a save, ASSIST-L19) may still have been applied on the server; the page's `base` is then stale and the next save of that cell answers 409, the existing conflict behaviour.
- **ASSIST-L34** [models] The GPU path is not built in this release (step 2c-2b): the assistant runs on the CPU build, at level 1 (the only level used, ASSIST-43), whatever GPU the machine has.
- **ASSIST-L35** [models] A failed start names a fixed cause (model not loaded, exited with its status, sandbox not set up), not llama-server's own message: the drain keeps no line of the child's output, by design (no prompt text can reach Mimik's log).
- **ASSIST-L37** [tests] The level-1 adversary test does not attack the sandbox's PID-1 reaper from inside (its memory, signals or tracing); it checks that the shim's status, seccomp, token, socket-directory, marker and runtime descriptors are closed in the executed process. Two regressions have no failing-first run: the sweep's real wait for a killed leftover (the kill completes before the check can observe it; the still-running outcome is tested through an injected wait), and a job superseded exactly as its run deadline fires (the scheduler did not produce the simultaneous wake in 8 runs; the fix also covers it).
- **ASSIST-L38** [security] (Level 3, not used in this release, ASSIST-43.) Only level 3's runtime path has its ancestors checked (each owned by root or Mimik's user and writable by no one else). For the data and models folders `open_dir` checks the folder itself only (its owner and mode), not its ancestors; their files are opened beneath that folder's descriptor without following links, and the model is hash-verified.
- **ASSIST-L41** [models] A record interrupted after its write but before its truncate, or a disk error mid-write, can leave a torn pidfile entry (it carries a SHA-256 line, so it never parses as a different child). The next start refuses with "the pidfile cannot be read" and leaves the generation directories, until the owner removes the file; the next preflight then re-acquires.
- **ASSIST-L40** [security] (Level 3, not used in this release, ASSIST-43.) Level 3 loads its libraries from the verified build directory by descriptor (its inode, kept open across the exec as `LD_LIBRARY_PATH=/proc/self/fd/<n>` and as the cwd), and that directory is verified to hold exactly the pinned files. A library added to, or an existing one replaced in, that same directory between the verification and the exec, by Mimik's own uid or root (the only writers its mode allows), would still be found by ggml's backend scan; a compromised same-uid writer is already outside the threat model. Closed by loading from a private directory of hard links to the verified inodes.
- **ASSIST-L39** [models] A Restart or shutdown waits without a deadline for in-flight cleanup: a runner or reaper stuck in uninterruptible sleep (D state) keeps the assistant "starting" and delays Mimik's exit until it ends; the log says so after 5 s.
- **ASSIST-L36** [models] Unverified on other hosts: the official CPU asset on the documented minimum host (an Ubuntu 22.04 VM), level 1 under Ubuntu 24.04's default AppArmor user-namespace restriction (CI records the probe's outcome with it on, without gating; level 1 is certified with it switched off), and the upstream bubblewrap fallback of the `llama-runtime` job (the 24.04 package has every option, so the fallback has not run).
- **ASSIST-L20** [models] The CPU path may not meet its caps with the default model. Granite 4.2 3B has `max_tokens` 352 under the adopted prefix, so generation dominates T(B). The first native run (level 1, 5 threads, 2026-10-22, while a system update compiled on the same host, so not the owed idle calibration) measured about 59 ms per prompt token and 184 to 206 ms per generated token, a 7-token uncached tail, and derived "too slow on this machine: about 105 s needed, the cap is 90 s" (`docs/results.md`). Closed by the idle native calibration at the ASSIST-41 default and at 2 threads, and the user's answer if it still fits nothing.
- **ASSIST-L21** [models] The 2b container topology is dev and benchmark evidence only and is not shipped (ASSIST-29). In it the forwarder is not isolated: it reaches host addresses and resolves external names through Docker's embedded resolver; a fixed-upstream socat limits what it does, not what it can do. The production levels replace it: level 1 is live-tested on the dev host and in the `llama-runtime` CI job; level 3's switched-off code is unit-tested in the container (`docs/model-runtime.md`).
- **ASSIST-L22** [models] Literal provenance checks literals only (ASSIST-38): constants derived from references pass it, so `hp ÷ ((hp + hp) ÷ hp)` halves without an admitted 2. The option is still fully shown and needs confirmation. Closed by a sound structural rule if one is adopted.
- **ASSIST-L23** [models] The model server has no memory cap: `RLIMIT_AS` is unusable with GPU drivers' address-space mappings, and cgroup limits are not used. The preflight only refuses a model file larger than available memory minus 1 GiB (`docs/assisted-sheet-plan.md`, §1.4a). Closed by a cgroup v2 memory limit if one proves workable with the drivers.
- **ASSIST-L24** [models] Confinement level 2 (Landlock, ASSIST-34) is not built in this release (step 2c-2b), and level 3 is not used (ASSIST-43): on a machine where bubblewrap cannot run, the assistant is unavailable (the page says how to enable it). When built, its tests need a kernel with Landlock ABI 4 or later (the dev host's has none, nor may a CI runner), so they are a host or VM measurement, and until they have run the level is unverified (also stated in the release scope).
- **ASSIST-L25** [models] (2c-2b.) Without bubblewrap (confinement level 2, or level 3 with the owner's explicit permission, ASSIST-43) render nodes cannot be isolated per device, so the GPU is used only when exactly one hardware GPU is visible; a multi-GPU machine without bubblewrap runs the assistant on the CPU (`docs/assisted-sheet-plan.md`, §1.4a, Device). Closed by a Vulkan device-identity probe that maps a device to its render node without isolation.
- **ASSIST-L26** [models] On the bubblewrap level the per-generation socket directory under the data directory is writable by the model server: each file is bounded by `RLIMIT_FSIZE` but their number is not, so a compromised server could fill Mimik's data disk (not the user's session). Closed by a size-limited mount for that directory if bubblewrap can provide one.
- **ASSIST-L28** [tests] The worst-case list re-render tests (`list_tests.rs`: refused cell and new-row drafts, a new row kept from a full list) run close to the auth layer's 5 s request timeout in debug builds on 2 cores and fail intermittently with 408, on master as well; serde_json's `preserve_order` (DEP-18) adds about 8% to them. Closed by measuring them in a release-profile build or with a test-only timeout.
- **ASSIST-L29** [script] The sheet module starts an interpretation only from a save's JSON answer on that page: a scribble already on the page when it loads gets none until it is saved again. With the assistant off, the first scribble of a page posts one start that answers 404; the module then stops trying.
- **ASSIST-L32** [models] Field-reference interpretation is unreliable in the screen: Granite referenced another field in 4 of 6 field-reference cases under v1 and in 0 of 6 under the first v2 prefix (none correct either way under v2); the alias scheme is not shown to be the cause, since the instructions, examples and prefix changed together. It gates the 2d default-model choice, not 2c-1 (TODO.md lists the follow-up experiment).
- **ASSIST-L3** [pages] The scribble excerpt under a cell is cut at a character boundary and can split a grapheme cluster; cosmetic. Closed by a segmentation crate if one is otherwise needed.
- **ASSIST-L4** [models] The model quality bar is waived for the POC (D-D); the assistant is not certified. Revisit before release (a known-answer set is the proposed route).
- **ASSIST-L5** [storage] History has no cap and each changed Enter writes a revision (D-U), so storage is unbounded. A per-account write-rate limit or storage guard is required before release.
- **ASSIST-L6** [busy page] The busy page cannot read the pin, so it names fields by ID and offers enum or yes/no entries as text. Up to 8 requests can queue 2 s each for the global permit; no per-account fairness.
- **ASSIST-L7** [busy page] A body slower than about 3 s plus the 2 s wait hits the 5 s timeout (408, not the busy page); the edit-form parser runs outside the permit and can outlive a cancelled request.
- **ASSIST-L14** [busy page] The busy page shows no validity flag (RR-3): it reads no sheet without the runtime permit, so the flag appears only once the sheet is reloaded.
- **ASSIST-L8** [tests] No browser test of the busy page (it would need a test hook in the shipped binary); covered through the full auth router.
- **ASSIST-L9** [history] Undo and history show enum option IDs ("was guard"), not labels.
- **ASSIST-L10** [entry] The switch offer lasts until the cell changes (no dismissal records); step 2c must not rely on its lifetime or treat a shown offer as confirmation.
- **RR-L1** [history] The history list does not show each revision's validity flag (ASSIST-19): open a revision to see it. A per-row flag would decode and evaluate every listed revision (up to 50 per page, each up to the 500 ms deadline) or needs a bounded cached result.

## Formulas and sheet values

- **F4** [formulas] Declared inputs are decided twice (`formula_admission::undeclared_inputs`, `formula_spec`), kept in step by a test. Derive the projection from `formula_spec`.
- **F5** [formulas] `campaigns::web::rejected_editor` with `permit: None` can answer a bare 503 that loses the rejected draft; untested. Keep the permit or preserve the draft.
- **F-HOOKS** [tests] Process-wide test hooks (`domain::DEPARTURE_PINS`, `web::REJECTED_EDITOR_KEPT_PERMIT`, `web::AFTER_FAILED_SAVE`) silently rely on `definitions::TEST_RESOURCE_LOCK`. Document the lock or scope per test.
- **F-NULLCASES** [formulas] The conformance fixture has no positive cases showing nullable text, enum and boolean fields keep CEL null semantics (its only nullable field is integer `luck`). Closed by adding them (a fixture content revision under D-Y).
- **F-NULLPROP** [formulas] `convert::unknowns` treats any `null` item property as unknown; fine while only integer properties can be `null`. Narrow it in the same change that admits nullable non-integer list properties.
- **F-DEPTH** [formulas] The expanded-AST depth limit (64) rejects flat operator chains of more than about 60 terms (`depth_exceeded`); authors split long sums into derived fields.
- **F-NONDET** [formulas] `deadline_exceeded` and `internal_error` are non-deterministic, so history can differ under load; an abandoned request keeps the JOBS permit until its blocking step ends.
- **F-EDITABLE** [lists] Every list property is editable, including stats that should be fixed; no `editable: false` and no item identity beyond row slots.
- **F-PERMIT** [lists] On the create POST the JOBS permit is taken before the 5 MiB body is buffered (runtime middleware), so any signed-in principal can hold it up to 5 s; edit and unit saves already buffer, parse and check CSRF first. Closed by taking create's permit after buffering.
- **F-TIMEOUT** [lists] The 5 s timeout covers the body: a 5 MiB POST needs about 8.4 Mbit/s; slower clients time out and nothing is saved (realistic list forms are about 100 KB).
- **F-SLOTS** [history] The change summary describes row slots, not items, and shows no values (RR-10 allows them): an overwritten row reads "all properties changed". Value diffs, revert from history and an index summary are not built.

## Runtime definitions, registry and publication

- **DEF-L1** [definitions] Controls are text, multiline, integer, boolean, enum and flat lists only: no decimals or nested groups and lists. Each extension needs a reviewed contract change (`items.quick_add` is the latest, ASSIST-13).
- **DEF-L2** [definitions] No cross-version migration of saved values; a sheet stays on its pinned version. Closed by a reviewed explicit migration operation.
- **DEF-L3** [definitions] Admission cost was measured only on small samples without sources (worst about 54 ms, 17,688 KiB RSS); 32-source, 1,024-reference bounds are uncertified. Closed by a worst-case profile.
- **DEF-L4** [definitions] Rule legality is advisory only: no minima, maxima, prerequisites or transitions are enforced, and final-state constraints cannot check transitions. Closed by the definition-language work (DL-L1).
- **DL-L1** [definition language] Not designed: entity references, tables, choices, actions, modifier stacking, grants, events and durations, box tracks, prerequisite graphs, picks, cost tables, rolled values, tags, campaign and scenario scope. Needs its own plan, starting with prior art (CEL extensions, Foundry Active Effects, GCS, PCGen, Datalog) and a `formula_eval` harness.
- **DL-L2** [definition language] Model authoring of definitions and catalogues from manuals is unmeasured. Closed by a pre-registered measurement on held-out pages with owner review time as the primary cost and the model never on the correctness path.
- **REG-L1** [registry] `list-rulesets` and `/admin/rulesets` always report `generic_sheet_available: false` (`src/registry.rs` `read_entry`), although published definitions enable generic sheets. Compute it or remove it.
- **REG-L2** [registry] Version-1 legacy sources and characters whose label has no alias stay unresolved until an exact alias is registered.
- **REG-L3** [registry] The `registry_events` trigger cannot tell when an edition was created, so direct SQL can add an event for a pre-history edition. Accepted.
- **PUB-L1** [publication] Comparison output links `/sources/pages/...` (only `--no-auth` mounts it); Owners must use `/admin/materials/pages/...`. Closed by RR-L4.
- **PUB-L2** [publication] Approval and withdrawal actor and rationale are free local labels; CLI rows carry no account attribution. Closed by RR-L4.
- **PUB-L3** [publication] Rulesets, drafts, versions and approvals cannot be deleted or retired (insert-only triggers), although CAMP-13 allows deleting unreferenced ones. Closed by a reviewed retirement or deletion operation.
- **PUB-L4** [publication] Integrity relies on SQL triggers plus Rust re-verification; no protection against an administrator replacing the database or dropping triggers.
- **PUB-L5** [publication] Wiring approval checks layout mapping only; it does not certify manual fidelity, unit review, page completeness or game rules.

## Catalogues

- **CAT-L1** [catalogues] The profiler's real-lock test records "released" before COMMIT; the R8 bound does not depend on it, but a controlled commit-phase regression test is missing.
- **CAT-L2** [catalogues] The worst browse page (50 entries × 16 units of 256 KiB) takes about 0.42 s, above R8's 250 ms, because the badge probe scans each cited unit; a cheaper probe must stay fail-closed.
- **CAT-L3** [catalogues] No test interrupts the types, cursor and badge-probe statements under the search budget; only name search and plain browse are tested.
- **CAT-L4** [catalogues] Unmeasured against R8's p95: real-route page time, untyped browse sorting up to 6,144 rows, and up to about 24 `latest_sealed` lookups per sheet render.
- **CAT-L5** [catalogues] Lock margin is thin and host-specific (worst hold 1,804 ms of 2 s on one host); re-measure on target hardware before raising any cap.
- **CAT-L6** [catalogues] Catalogue routes are not rate-limited; a client looping on searches holds the runtime permit and makes sheet saves answer 503.
- **CAT-L8** [catalogues] The two 20,000 limits are unchecked against real catalogues: the entry text cut (`DISPLAY_CHARACTERS`, `src/catalogues/read.rs`) and the owner-text cap (`MAX_OWNER_TEXT`, `src/catalogues/decode.rs`). The owner asked to inquire whether they are valid.
- **CAT-L10** [catalogues] An invalid search (`q=**`) wins over an unknown `type`, answering the 422 search problem instead of the 404 (reveals nothing); revisit with the assisted-sheet work.
- **CAT-L11** [catalogues] A logged-out browser following a malformed catalogue link gets the 400 page, not the sign-in redirect (syntax check runs first).
- **CAT-L12** [catalogues] The publication crash test assumes the rollback journal; re-check it with WAL (C6).
- **CAT-L13** [catalogues] Labels use Unicode 16.0 categories, so a later-assigned Cf passes; refusing Cf also refuses ZWNJ, ZWJ and soft hyphen in names.
- **CAT-L14** [catalogues] Append-only catalogues grow forever: a mistaken entry can only be retired, and history pages show today's description for an old key.
- **CAT-L15** [catalogues] `verify-catalogues` cost grows with history (about 8.4 s for 13 versions of up to 6,144 entries).
- **CAT-L17** [catalogues] Publication certifies references, not text: unreviewed raw text can be garbled (the badge says so), and a description cannot cite part of a unit.
- **RR-L2** [limits] Capacity caps are still product caps (RR-8): 128 campaigns, 32 per creator, 64 members, 2,048 sheets, 32 per custodian (`src/campaigns/`, migration 012), 6,144 catalogue entries (D5; the user deferred raising these until the POC works). Replace with safety-only bounds and re-measure the departure freeze against the 5 s timeout.

## Campaigns and accounts

- **AUTH-L1** [accounts] A disabled account cannot be re-enabled and a disabled Owner cannot be recovered or replaced; the CLI lets the Owner disable itself. Closed by AUTH-12 and AUTH-13.
- **AUTH-L2** [accounts] Login and display-name changes and deletion of pending reservations are not exposed; expired reservations are kept forever.
- **AUTH-L3** [sessions] Throttling and admission are process-wide (one quota, 8 requests, 1 KDF job), not per user or client.
- **AUTH-L4** [sessions] A timeout or cancellation can follow a committed write, and a lost login response can leave an unreachable session row until expiry; clients re-read before retrying.
- **AUTH-L5** [security] Zeroization covers Mimik's own password buffers only (not swap, browser copies or backups); no constant-time login guarantee is claimed.
- **AUTH-L6** [security] SQL guards protect against ordinary application SQL, not against direct write access to the database file.
- **AUTH-L7** [accounts] Credential-file durability is verified only for Linux local filesystems with working fsync, not power loss.
- **AUTH-L9** [deployment] Behind a proxy, Mimik sees only the proxy; client network restrictions depend entirely on the proxy configuration.
- **AUTH-L10** [security] `--dev-owner-play` still lets the Owner play through SQL and route guards (AUTH-3). Remove it (`src/main.rs`, `src/auth/`) as its own reviewed change, deciding what happens to development Owner memberships and custody in existing databases.
- **AUTH-L11** [tests] `cli_owner_reset_can_complete_in_browser_and_existing_session_forms_are_readonly` (`src/campaigns/administration_tests.rs`) can get a 408 under full-suite load: real argon2 runs inside the fixed 5 s request timeout (`src/auth/mod.rs`), which tests cannot inject. It passed in the 2026-10-19 full run. Closed by an injectable timeout for tests.
- **CAMP-L1** [campaigns] Transfer, frozen-sheet reactivation, soft delete, Owner restore, purge and rollback are decided (CAMP-9 to CAMP-12) but not built; nothing can be deleted. Rollback semantics are undecided: ask the user when the lifecycle slice is selected.
- **CAMP-L2** [campaigns] The creator cannot leave, be removed or hand over the campaign; no creator succession.
- **CAMP-L4** [campaigns] Prototype character records cannot move into campaigns, and changing a campaign's ruleset or moving a sheet across rulesets has no policy.
- **CAMP-L5** [campaigns] The native MÖRK BORG adapter and runtime sheets are not certified official sheets or legality engines.

## Storage, platform and release

- **C6** [storage] One SQLite connection behind a mutex (`src/store.rs`), no WAL or pool, so imports, publications, lookups and large blob reads block other requests. Closed by evaluating WAL and a pool under the LAN workload.
- **C9** [storage] Migrations are embedded SQL applied in order (`MIGRATIONS`, `src/store.rs`) with no down-migrations, and immutability relies on large triggers. Evaluate a maintained migration crate or record why the scheme is enough.
- **CORE-3** [release] Verified only on Linux x86_64, trusted IPv4 LAN, root-origin proxy and Chromium desktop/mobile viewports with synthetic inputs; phones, IPv6, subpath proxies, public hosting and other platforms are uncertified.
- **CORE-4** [CI] No hosted `deployable` run on the GitHub remote, and no ruleset requiring it, has been confirmed; local runs are the only evidence. Closed by a green hosted run and the required status (pushing needs user approval).
- **CORE-5** [licensing] Authorship and redistribution rights of the root `logo.png` are unconfirmed; the owner must confirm them before a public release.
- **L-LIC-2** [licensing] Marker's weights carry modified OpenRAIL-M terms, read only from the upstream README; check the installed version's code and weight terms before using its output beyond local preparation.

## Sources and retrieval

- **L-SRC-1** [sources] OCR, layout and full-manual fidelity are unverified (one unreviewed Marker page; `--ocr auto/force` and AMD/Vulkan untested). Closed by selected-page benchmarks with pinned artefacts and visually resolved discrepancies.
- **L-SRC-2** [sources] Scalable whole-book admission is an open policy and design question: model-checked material cannot become verified lookup without human review, no automated-coverage policy exists, and book-wide hierarchy is not in the schema. Italian Pathfinder errata coverage is unknown.
- **L-SRC-3** [sources] Imports accept only zero-rotation, zero-origin pages, reject nested table or form groups and `<content-ref`, and allow 64 pages and 20,000 units per run.
- **L-SRC-4** [sources] `tools/prepare_review.py` writes contract v1 only; a v2 bundle must be written by hand.
- **L-SRC-5** [sources] The extraction image lacks `python-dotenv`, so `review_extraction.py` reads its key only from the environment, not a mounted `--env-file`.
- **C5** [sources] Every bundle stores the full PDF (up to 128 MiB) and `pdf_hash` is not unique, so a manual imported in several 64-page runs is stored several times. Store each PDF once per hash.
- **C8** [sources] Page dimensions live only in `prepared_json`, so every region crop scans up to 24 MiB. Store them as columns.
- **C10** [sources] Import validates polygon bounds but not degeneracy or point order; the crop rejects only zero-area rectangles.
- **L-REG-1** [sources] Region previews reject cICP, mDCV and cLLI and reduce 16-bit to 8-bit; the 512 MiB per-job figure is unmeasured.
- **HIER-1** [evidence] `tools/evidence_forest.py` validates supplied multi-book structure only; it does not discover hierarchy and is not connected to the source store.
- **LEG-1** [evidence] Legacy Pathfinder chunks lack page, region and review identity and risk truncation; any replacement first needs failing-first regressions.
- **L-RET-1** [retrieval] bm25 statistics are table-wide: reviewing or importing other editions can change top-50 membership within an edition. Closed by per-edition tables or re-ranking.
- **L-RET-2** [retrieval] Phrases can match across flattened table cells, list numbers are indexed as tokens, and mid-word substrings do not match; a trigram side index is the deferred remedy.
- **L-RET-3** [retrieval] Lookup p95 is 32 ms at 15,000 units but 207 ms at 100,000 adverse units (target 50 ms); the 1 s `QUERY_BUDGET` bounds VM work only, not one long FTS5 `xFilter` call.
- **L-RET-4** [retrieval] `Store::open` misses stale index content when counts match, `rebuild_search_index` has no CLI, and a large rebuild can exceed the 5 s busy timeout.
- **L-RET-5** [retrieval] Unsanitizable legacy HTML cannot be shown in the page view; such units are repaired only by re-import or the store API.
- **L-RET-6** [retrieval] Lookup is reachable only from the `--no-auth` page `/sources/search`; no authenticated UI, no source-set UI, no campaign or tool-loop caller.
- **L-RET-7** [retrieval] No vector, hybrid or reranked retrieval and no recall@k set; Italian queries do not match English text. Blocked on a multilingual embedding model and a gold-labelled set over a full corpus.

## Models and evaluation

- **C1** [models] The release ships the model runtime but no model file: the owner places the GGUF file in the models folder by hand (downloads are step 2d-2); there is no tool loop.
- **MOD-1** [models] All model results so far are synthetic (Lantern Reach, 18–28 cases). Closed by source-checked MÖRK BORG and Pathfinder held-out cases, separating extraction, retrieval, context and reasoning failures.
- **MOD-2** [models] The broad screen is unfinished: Mapika-2B/4B, Tacit-4B, Kev-4B, Clef/Clef-Flash, DJev and Phi-4-mini were never run. Admit or block each with a reason.
- **MOD-3** [models] Calibration is untested on RPG data: probabilities ≥ 0.9 were wrong in 8–28 of 58–64 observations and option order flips winners. Do not gate on confidence without held-out calibration.
- **MOD-4** [models] The AnyJev GGUF backend clears the KV cache before every prompt, so warm prefix reuse on GPU is unmeasured; Kev's pointer head is verified only on CPU FP32.
- **MOD-5** [models] Prompt-injection resistance is not established (a thinking 4B followed injected notes). Closed by an isolated injection test set.
- **MOD-6** [tools] The live deadline-stop path of `tools/fetch_models.py` is untested; unit tests cover only arithmetic and phase order.
- **TL-COV1** [tool-loop pack] No case covers "apply a current pending proposal" because the pack never defined it. Closed by defining it in the pack README and adding dev cases.
- **TL-COV2** [tool-loop pack] Four ambiguity classes ("Known open ambiguities" in the pack README) are undecided and outside the blind writer's scope. Closed by user decisions plus relabels.
- **TL-L1** [harness] Expected-outcome replay copies `answer_values`, so it cannot validate answer gold; recompute answers independently in the gold check.
- **TL-L2** [harness] A finishing call written as plain text is `no_outcome` by design, so template and parser fit dominates small-model scores.
- **TL-L3** [harness] Message prose is not graded; the int hedge (a stray extra int) escapes the hedge guard and is only counted as `int_extra_verified`.
- **TL-L5** [harness] `propose_change` cannot add or remove entries or set an optional property to null.
- **TL-L6** [harness] Results with fewer than 100 held-out cases, or one seed, cannot rank models.
- **TL-HO1** [held-out] The held-out gold is agent-written and agent-reviewed. Closed by a user sample review.
- **TL-HO2** [held-out] The pin and hash chain are self-asserted, not tamper-evident. Accepted for a single-user host.
- **TL-HO3** [held-out] With 78 at-risk cases even 0 false support has a Wilson upper bound of about 4.7%, so the set can refute but not confirm the 2% bar.
- **TL-T1** [runs] A stalled Docker daemon can delay removals past the window edge; set `--session-end` early.
- **TL-T2** [runs] Containers are addressed by fixed names; diagnostics are not label-checked. Accepted for a single-user host.
- **TL-T3** [runs] A second clone's `target/` locks are invisible to this clone's watchdog.
- **TL-N9** [runs] Every stop and kill goes through the docker CLI; no direct cgroup or process kill.
- **TL-SCHED** [runs] Runners refuse all heavy steps on weekday hours and give CPU servers 6 threads; add a 2-core weekday mode for small CPU models in `tools/toolloop_ops.py`.
- **TL-THERM** [runs] CPU temperature is unreadable on this host (no `k10temp`), so CPU arms have no thermal stop.
- **ROUTER-L1** [router] `decision_router.py overlay` keeps totals only and does not assert ledger ids against the subset.
- **ROUTER-L2** [router] `select --router` is not checked against `bindings.runtime.router_id`, and the report omits the code-identity hashes it checks.
- **ROUTER-L3** [router] The AnyJev worker reports `llama_supports_gpu_offload: False` on a Vulkan build; reconcile before any GPU router run.
- **ROUTER-L4** [router] `tools/run_decision_router.sh` has `HARD_DEADLINE` fixed at 2026-10-19 09:00 and refuses every run since; a new run needs a new pre-registered deadline.
- **ROUTER-L5** [router] Cosmetic docstring and field mismatches in `tools/decision_router.py` (`per_fold_strata`, `outcome_options`, a missing `verify-decision-file`).
- **ROUTER-L6** [router] Folds are grouped by metadata only, so text near-duplicates across folds are possible.

## Later

- **CAT-L7** [catalogues] (later) Catalogue administration is CLI-only with no withdrawal command; Owner browser pages over the shared transitions are allowed (PROD-18).
- **RR-L4** [definitions] (later) Definition publish and withdraw are CLI-only (PROD-18). Phase B `/admin/definitions` keeps CLI output byte-identical, attributes `account:<id>` via insert-only web events and pins the ticket hash.
- **CAT-L9** [catalogues] (later) Sheets cannot store catalogue references (catalogue step 4, shelved 2026-10-19); if resumed, an unknown key warns (RR-2).
- **CAT-L16** [catalogues] (later) Manual page images are not shown to players beside catalogue entries (Q1); allowed after the assisted-sheet POC.
- **RR-L5** [formulas] (later) Derived values are read-only and cannot be overridden by the player (RR-7); needs a definition-level override design with its own review.
- **RR-L6** [sheets] (later) Controls are fixed per value type (booleans are unknown/true/false selects); no custom per-field input controls (RR-7).
- **PROD-L2** [sheets] (later) Sheets are generic editors: no official layout mapping, print or export, no layout fidelity tests.
- **PROD-L3** [play] (later) No situation facts or intent capture, no options with costs and prerequisites, no house-rule overrides; dice are notation only; `src/mork_advice.rs` is the only contextual advice.
- **PROD-L4** [expansion] (later) No plugin or pack system (manifests, declarative packs, WASM or subprocess adapters, loading with compatibility checks, author guide, conformance tests); shared abstractions are unvalidated across games.
- **C11** [games] (later) MÖRK BORG mechanics and IDs are hardcoded (`src/mork_borg.rs`, `mork_advice.rs`, `mork_form.rs`, matches in `campaigns/`, `games.rs`, `lib.rs`); accepted as an evaluation fixture, but in conflict with PROD-5 for new games.
- **ROAD-1** [roadmap] (later) A versioned rules-adapter contract with capability and coverage reporting on top of `src/registry.rs`.
- **ROAD-2** [roadmap] (later) A first playable uploaded game package with official-layout rendering and export, and model dialogue over an owner-supplied template.
- **ROAD-3** [roadmap] (later) A situational companion: options with costs, prerequisites and cited explanations, house-rule overrides, GM adjudication.
- **ROAD-4** [roadmap] (later) Hybrid retrieval benchmark, typed provider interfaces, finalist comparison, native readout parity, broad vendor support, grounding and latency on real manuals.
- **ROAD-5** [roadmap] (later) Remaining D&D, Pathfinder, Vampire and MÖRK BORG editions, each with a published field, rule and action coverage matrix.
- **ROAD-7** [roadmap] (later) Rollback beyond per-cell Undo, campaign-private knowledge, sheet import/export and Foundry interoperability, backups, localisation.
