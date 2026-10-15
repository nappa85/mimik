# Bounded hierarchy and local screen execution

2026-10-15. User authorizes execution while away for about one hour and clarifies
that game materials form a multi-book corpus. Plan reviewer
`ses_ec045063effeU7mOiNbk5rsbzF` approved the amended limits and source-set forest.

Authoritative scheduling amendment after observed ~3MiB/s transfer: original start
13:26:46UTC, finish14:21:46UTC; model inference hard stop13:50:46UTC even in flight.
Setup/download<=1200s aggregate, per candidate<=900s; sequential screen<=1200s,
all subject to the absolute cutoffs, not extra time. Additional weights<=12GiB,
reserve20GiB. Reviewer approved maintained aria2 transport with<=8segments/artifact,
LFSsha verification and bounded retries. Slow HF transfer was stopped before
changing transport; no partial/unverified model may load. These limits supersede
the initial phase/per-setup bounds below. Numerical inference cap remains384.

Optional Python prototype only, using existing jsonschema4.25.1 and stdlib parent
traversal. JSON Schema provides shape validation; custom code only handles Mimik
source-set/version identity, structural ancestry, references and invalidation.
Generic tree libraries (e.g. anytree) do not supply those source semantics and add
no necessary traversal capability for this bounded graph; no new tree/database
dependency. No PDF extraction/inference of hierarchy or shipping schema change.

Source-set forest: enabled exact book/revision pins, same-edition explicit policy;
each book retains its own tree. Structural parents cannot cross books. References
can cross enabled, accessible books, with missing/disabled targets represented but
not disclosed. No inferred expansion precedence, title matching or GM visibility
grant. Node identities survive reparenting/title edits. Unresolved structure stays
incomplete; dangling resolved parents fail. All known endpoints still validated.
Canonical input fingerprints scope derived artifacts to forest, source-set and
policy; access is checked at assembly, not granted by a cache fingerprint.

Write meaningful semantic tests against a functional flat baseline, establish red,
then implement fixes and retain regressions. Include 100+page continuation,
same-name entries, source-set changes, multibook reference isolation, editions,
class/subclass boundaries, cycles, span containment and repair staleness.

Hard scheduling limits after plan admission: hierarchy <=900s; sequential metadata/
screening <=1500s; reserve >=900s for checks/report/fresh review, <=3300s aggregate.
Six executed candidates are an attempt target, never a promise. No new setup after
screening deadline; report partial/unattempted separately. Prior results historical
only. Per candidate setup<=600s/load<=180s; each whole logical decision<=60s including
all internal computation. At most384 logical AND physical evaluations aggregate;
max48scored/candidate, no adaptive reasoning/retries/L0 priors. Actual raw readout
one forward/decision; warmups/parity checks count against physical cap.

Sequential models, CPU memory/swap<=24GiB,6threads; GPU host freeRAM>=12GiB and check
VRAM admission, stop on allocation failure, no silent CPU fallback. Additional
downloads<=24GiB, scratch<=40GiB, freehome>=20GiB checked before setup/download.
No host installs/tmp build caches. Only explicit public weights/local fixtures and
output mounts. Inference offline/no published ports/no credentials/private corpus.

Initial new candidates Mapika2B native helper and Tacit4B trained native helper;
existing3QwenGGUFs+Kev0.8 native reference provide up to6 attempts. Phi only if
availability/runtime prevents a planned new candidate, never selected based on
accuracy; role/config separately reported. Clef/Flash/DJev/Kev4B assessed for faithful
runtime fit, no generic-backbone substitution. Pin hashes/license/code before runs.

Freeze/hash 18existing development cases plus6new hierarchy/source-scope cases,
options/canonical mapping, scorer/version/config/timeouts before calls. Two orders,
one repeat=48observations over24unique cases, sixcase stratum separately reported.
Hierarchy supplied as oracle evidence tests use of structure, not discovery. Gold
labels evaluator-only. Required missing/contradictory/unresolved cases included.
No general RPG reliability/production readiness claim. Permanent regressions,
helper lint/tests/package allowlist and fresh implementation/results review gate
completion; if incomplete preserve evidence and explicitly hand off blocked work.
