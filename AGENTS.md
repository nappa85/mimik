# Mimik project rules

- Start each session with `TODO.md`: current work, next steps and open blockers only. Keep it short; remove items once done.
- Commit locally without asking (user decision 2026-10-16): local history can be rewritten. Pushing, or anything else that leaves the machine, still needs explicit approval.
- **Machine schedule (user decision 2026-10-16):**
  - Monday to Friday, 09:00–18:00: no heavy operations (GPU inference, model loads, uncapped builds). Rust builds and tests are allowed during those hours, capped at 2 cores.
  - Weekends: the GPU and heavy work are allowed 24 hours a day.
  - Small CPU models (about 4B parameters or less, quantized, no GPU) may run during weekday hours too, capped at 2 cores like builds (user decision 2026-10-19).
  - Model downloads are allowed without asking after licence and compatibility checks; store them under `/media/data/mimik` (slow magnetic disk, plenty of space), pinned and hash-verified.
  - Unattended runs must enforce the window themselves and never wait for input.
- Prefer existing maintained crates/libraries over reinventing infrastructure. Before custom implementation, check fit, license compatibility, maintenance, API/capability coverage, portability, and dependency/runtime cost. Record meaningful selection decisions; custom code should cover Mimik-specific behavior or demonstrated gaps.
- Rust is the application/shipping language. Python is acceptable for extraction/evaluation prototypes. Keep optional prototype dependencies separate from the application.
- Final provider support should use an established Rust abstraction, not hand-written vendor protocols. Vendor expansion is a roadmap item; keep provider capabilities and model roles explicit.
- Recreate official sheet structure in adaptive HTML/CSS, retaining recognizable field grouping while allowing long text and mobile reflow. Canonical data is independent of layout.
- Use deterministic, source-backed rules for calculations and state changes. Model output cannot establish legality or automatically alter sheets.
- Preserve layout-aware extraction tables/regions/metadata and compare used evidence against original PDF images. API review is supplemental; no automatic approval from model confidence or remembered rules.
- Manuals, extracted corpora, rendered source pages, databases, and secrets remain local and outside source releases. Do not print/read back .env contents or credentials; test with dummy secrets. Respect user-owned files.
- Run checks appropriate to changes. Distinguish unit/helper verification from live inference/provider or browser testing; never report unrun checks as passed.
- Install new tools, prototype dependencies, and model runtimes in containers, not on the host (exception, ASSIST-40: the pinned official llama-server may run natively for the runtime-manager tests). Keep build contexts allowlisted and never include secrets/manuals. Preserve the lightweight standalone Rust runtime.

## Documentation (user decision 2026-10-19)

- Docs describe the system as it is now: objective reference, not a log of actions. Git history is the log.
- Layout: `docs/decisions.md` (decisions in force, one line each with date; superseded ones removed), `docs/limitations.md` (known limits and follow-ups), `docs/results.md` (experiment and benchmark conclusions still in use), reference docs per area, and at most one plan per active piece of work containing only the remaining steps.
- When work lands, update the reference docs and delete the finished plan sections. No implementation logs, gate tables or red→green tables in docs; put them in commit messages.
- Project rules are means to an end. If a promising library or approach conflicts with one, state the conflict and its cost and let the user decide; security and data rules are not negotiable.

## Independent adversarial review

- Every plan must receive an independent adversarial review before execution, and every material implementation step/change set must receive one before being marked complete, except under the prototype tier below. Use a separate reviewer/subagent with a fresh context; self-review is supplemental, not independent review.
- Review correctness, missing assumptions, failure modes, security, source/rule fidelity, deployability, suitable existing libraries, duplicate logic, maintainability, and production practices. Prefer a read-only reviewer; the implementer owns fixes.
- Review outcomes are not kept as files (user decision 2026-10-19). Fixed findings live on as regression tests and code; the merge commit message states the reviewers, their verdicts and any Blocking/Major finding with its resolution. Unresolved findings go to `docs/limitations.md` or `TODO.md`. Review findings are not proof of correctness.
- **Reviewer setup (user decision 2026-10-16):**
  - Main reviewers: OpenAI Astra (`openai/gpt-6-astra` via `tools/external_review.py --chain main`; replaced Sol, user decision 2026-10-22) and a fresh Sonnet subagent (`model: sonnet`; Fable was tried and dropped for token cost, user decision 2026-10-22), run in parallel.
  - Third reviewer: the first that works of MiniMax M3.1 Flash (formerly Space Bunny), DeepSeek v4.1 Flash, then Beam (`--chain third`; user decision 2026-10-22 after reviewer benchmark 2). Free models have daily limits: never run them in parallel. If none works, proceed with the two main reviews and record that.
  - Adjudicate Blocking findings against the code before acting on them.
- **Implementers (user decision 2026-10-28):** Sol 6.1 (`openai/gpt-6.1-sol` through opencode), run by `tools/sol_agent.sh` in bubblewrap. It sees the worktree, the toolchain, a cache and the opencode login read-only; it cannot see secrets, other worktrees, git or the Docker socket; a system-call filter applies. The coordinator commits its work and answers its `ask` mailbox. Sonnet subagents are the fallback.
- Resolve blocking findings before completion. If independence is unavailable or a blocker cannot be fixed, explicitly record it and keep the work blocked; do not call it production-ready or silently waive it. Review of review records themselves does not recurse.

### Prototype tier (user decision 2026-10-15)

- Disposable experiments (benchmarks, evaluation scripts and prototypes under `tools/` or ignored `target/` paths that never ship in the Rust application or release artifacts) may use a lighter process: a self-check plus the conclusion, with exact scope, denominators and limitations, added to `docs/results.md`.
- Independent review is still required before experiment results drive a product or architecture decision, and before any prototype code moves into the shipped application.
- Security and data rules still apply in full: containers for new tools and runtimes, no secrets read back, manuals and corpora stay local, and checks are never reported as passed when not run. Shipped code keeps every gate.

## Regression and production quality

- For every reproducible issue found, add a permanent regression test that fails before the fix. Run it to establish the failure, fix the cause, then retain the test in normal CI. Do not ignore, delete, weaken, or mark an unresolved regression as expected failure to obtain green CI.
- Use maintained crates/tools for generic capabilities, remove avoidable duplication, use clear domain boundaries and structured validation, and avoid brittle ad-hoc infrastructure. Document demonstrated gaps that justify custom code.
- Do not expose secrets, suppress errors, or disable lints/tests/audits to hide defects. Any justified exclusion needs documented narrow scope and reviewer approval.
- CI is a fail-closed deployment gate: formatting, strict Clippy, locked tests, helper lint/tests, dependency audit, licensing/notices, release build, and unpacked-artifact startup/edit/persistence smoke checks must pass. Failures, skipped jobs, or cancellation cannot mean deployable.
- Clippy `all` and `pedantic` are enabled project-wide and denied in CI. Fix findings instead of broadly suppressing categories.
- Green CI means deployable for the explicitly documented supported release scope, not that unfinished features, untested platforms, or unverified manual/model integrations are certified. Update that scope and required checks as features ship.
