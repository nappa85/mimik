# Experiment and benchmark results

Results whose conclusions are still in use, with exact scope, denominators and limitations.
Product-relevant measurements come first, then model and benchmark results. Raw data (ledgers,
logs, reports) stays local under `target/` and is not in the repository. The tooling is
described in [evaluation-tooling.md](evaluation-tooling.md).

## Synthetic PDF worker corpus and memory (2026-10-31)

The generated set contains 24 hostile and six valid difficult cases. Each declares an
exact inspect refusal or render outcome. The valid set includes all 300 text pages, a
3000 × 4000 full-page image, 256 font objects and inherited rotations 90/180/270. The
object-stream cycle uses type-2 xref entries: page resources require object 9 in stream B,
whose Length requires object 10 in stream A, whose Length requires object 9 in B. Inspection
passes and rendering reaches this resource path; the two compressed objects have no
physical fallback. Inert actions render successfully with the host canary absent.
The coordinator's confirmation host run passed 29/29 worker checks and 49/49 llama checks, with
the level-1 probe successful; tiny-images rendered Ok and page-cycle had its declared
inspect refusal.

Peak RSS from wait4 on the naturally reaped runner and its waited descendants is a
conservative worker upper bound only for natural exits, not a dedicated MuPDF allocator
measurement. A test-only worker touching 256 MiB measured 286,852 KiB in the coordinator's
exact-profile run, establishing descendant rusage propagation. Every corpus run must
remain below 1,572,864 KiB (75% of RLIMIT_AS 2,097,152 KiB). Natural exits use wait4;
parent-killed runs use the worker's host `/proc/<pid>/status` VmHWM, sampled during execution
and immediately before signalling. The verified worker pidfd must remain live on both sides
of each read, excluding numeric PID reuse. A missing killed-worker measurement fails the
gate; a second 256 MiB touch-and-wait control checks the killed path. VmHWM covers the worker
through the last pre-kill sample, not the subsequent kill/reap interval; wait4 on a
parent-killed runner does not account reliably for its unreaped worker. Both figures are
reported separately. The killed 256 MiB control measured 287,308 KiB VmHWM. The killed hostile
renders measured 28,728 KiB (`flate-gib`), 19,212 KiB (`operators-50m`) and 19,084 KiB
(`shading-pattern`), all below the same bound. The input Vec and MuPDF's copied buffer are both present. `flate-gib`,
`operators-50m` and `shading-pattern` time out during render, as declared. The responsiveness
control also ends in a parent deadline kill while a second confined inspect succeeds;
HTTP-level responsiveness is reserved for slice 3b.

The `page-cycle` fixture is refused at inspect as Unreadable, MuPDF's bounded cycle
detection. The tiny-image render fixture uses 19,999 draws, below the 20,000 image-block cap
in `operations::text`; the former 50,000-draw input correctly returned TooLarge at that cap,
before output framing. Render-stage fixtures require inspect success and exact Ok or Timeout.
Every naturally exiting case rejects unexpected signal status; logs distinguish parent kills.

Generator tests re-exec one case at a time and check `/proc/self/status` VmHWM below 64 MiB,
output below 8 MiB and generation below five seconds. This avoids wait4's pre-exec inherited
parent RSS when other unit tests run concurrently. Full-flush DEFLATE block repetition
generates the GiB and 50-million-operator payloads without compressing expanded input in the
debug build; checksum tests cover zero and nonzero repeated blocks. These measurements cover
the deterministic synthetic profile and the local-manual check below, not fuzzing follow-up.

The ignored `live_local_pdfs_inspect_and_render_every_page` check accepts owner paths from
`MIMIK_LIVE_LOCAL_PDFS` (colon-separated), with `MIMIK_LIVE_RUNNER` selecting the real runner.
It inspects each file and renders every page in batches of eight with production budgets.
It retains no extracted text or images and writes no document output; summaries identify
paths only by a 12-hex SHA-256 prefix and contain page counts, total/per-page timings, both
RSS figures and inspect/batch outcomes. The sandbox still uses its normal temporary private
root and in-memory spec/tmpfs. In the coordinator's host run, the two local MÖRK BORG manuals,
EN Bare Bones and IT, passed inspection and every render batch: 76 pages in 6.2 seconds
(median 0.064 seconds/page) and 96 pages in 24.1 seconds (median 0.206, maximum 0.709
seconds/page). Peak natural-exit wait4 RSS was at most 136,588 KiB. The test reports sampled
worker VmHWM separately; this wait4 maximum is the supplied natural-exit upper bound, not a
dedicated worker allocator measurement. This conclusion covers these two local manuals only.

## Extractor screen on MÖRK BORG IT pages 22–39 (2026-10-29/30)

`mork-borg-core-it-2025`, PDF pages 22–39 (printed 18–35: creation, weapons, armour,
equipment, abilities, HP, combat, rest, improvement, powers, scrolls), in network-less
containers. Scored against a reference transcription made by eye from the page images of
PDF pages 23–30 (161 rows, one fact each, kept local under `data/extractions/gold/`).
"Intact" counts rows found whole inside one unit (ready to review as they are), "present"
rows found in the page's text in reading order, "fuzzy" the mean best similarity. One run each
(the Marker OCR run was repeated: same scores); one manual; scoring by a screening script,
not shipped.

| Extractor | Intact | Present | Fuzzy | Weapons (24–27) | Armour (28) |
|---|---|---|---|---|---|
| Marker 2.0.0 fast, OCR disabled (text layer) | 140 | 143 | 0.957 | 0/12 | 4/10 |
| Marker, `--ocr auto` | 140 | 144 | 0.965 | 0/12 | 4/10 |
| Marker, `--ocr force` (surya-ocr-2 on llama.cpp b11609) | 141 | 145 | 0.988 | 3/12 | 9/10 |
| the same, plus text-layer spellings (`crosscheck_text_layer.py`) | 145 | 150 | 0.989 | 5/12 | 9/10 |
| Marker `balanced`, `--ocr force` | 141 | 145 | 0.988 | 3/12 | 9/10 |
| Docling 2.139.0, text layer with text inside pictures | 62 | 129 | 0.962 | 2/12 | 4/10 |
| Docling, full-page RapidOCR (PP-OCRv5 Latin) | 53 | 121 | 0.958 | 1/12 | 5/10 |
| PaddleOCR-VL-1.6 pipeline (PP-DocLayoutV3, VLM on llama.cpp) | 92 | 92 | 0.864 | 5/12 | 9/10 |

- Without OCR, Marker drops text drawn over illustrations (page 25: no unit at all) and
  keeps the text layer's drawing order (page 28: rows of the armour list mixed). `--ocr auto`
  judges the text layer good and does not OCR. Forced OCR finds every weapon label as its own
  block and reads page 28 in visual order; its errors are mostly gothic display letters
  (Violenza read "Diolenza", Quanto "Duanto", Nove "Move", Servizi "Scrivizi"), which the PDF
  text layer inside the same box spells right: the cross-check proposed 52 such repairs
  (with typographic ones, ’ and –), none wrong on inspection except one that drops a printed
  asterisk.
- Forced OCR gives whole lists as one `ListGroup` block, which the importer then ignored
  (9 blocks, 2,532 characters on these pages) until SRC-10.
- What review must still do on the chosen route, from a full visual review of the 175 units
  (64 suggestions, `review_extraction.py`'s report format): merge numbers, names and dice that
  the art layout scatters (weapons, scrolls), fill gothic titles with no text (Punti Ferita,
  Poteri), restore lost words (one row of the critical-hit armour track, "un Peccatore"), and
  reject the OCR'd buyer watermark. One footnote (page 29) is in no unit and cannot enter
  review.
- PaddleOCR-VL lost most of page 29's equipment table and took 30 minutes (image encoder on
  the CPU); Docling's picture text is complete but split into single cells (page 29: 160
  items). GLM-OCR was not screened: it needs the same layout model as PaddleOCR and gives no
  regions on its own. Marker's own forced OCR could not run in the image until surya's
  llama.cpp server was supplied (`tools/ocr_server.sh`).
- GPU: the pinned Vulkan llama-server in a container (Debian Mesa 25.0.7 RADV) lost the device
  while encoding an image on the RX 6650 XT; with the image encoder on the CPU
  (`--no-mmproj-offload`) both OCR models ran without errors. Host Mesa is 26.2.5.
- Licences: surya-ocr-2 GGUF modified OpenRAIL-M, PaddleOCR-VL-1.6 and PP-DocLayoutV3
  Apache-2.0, Docling MIT with Apache-2.0 and CDLA-Permissive-2.0 models, RapidOCR Apache-2.0.
  surya's OCR error model is CC BY-NC-SA 4.0 (local prototype use only).

**Conclusion:** extract with Marker and forced OCR through `tools/ocr_server.sh`, cross-check
against the text layer, then review every unit against the page image. Not measured: other
manuals, other pages of this one, the time an owner needs to apply the suggestions.

## Model trials of the downloaded default files (2d-2 live check 3, 2026-10-29)

Both pinned files as downloaded through the app from the real Hub (2d-2 live check 2), tried
through `/admin/assistant` by `tools/assist_download_live.py`'s trial path on the release
binary, pinned llama.cpp b11609, level 1, this host (Ryzen 5 1600, 6 cores / 12 threads, Zen 1;
RX 6650 XT with Mesa RADV, fan on manual 255), evening, with the owner's desktop in use. One
host, one run per row unless stated.

| File, path | Threads | Result | Priming | Samples W (prompt, generated 352) | Budgets |
|---|---|---|---|---|---|
| Granite 4.2 3B, CPU | 2 | too slow (a sample over the 90 s cap) | | | |
| Granite 4.2 3B, CPU | 5 | 1 of 3 runs too slow; 2 passed | 70.3 s, 70.8 s | 58.4 to 61.2 s (11.4 to 12.4 s; 46.7 to 48.7 s) | B 1,024, N(B) 280, R 75 to 80 s, D 1 |
| Granite 4.2 3B, CPU | 6 | passed | | | |
| Qwen3.5-9B, GPU | 6 | passed in 114 s | 11.8 s (1,961 tokens) | 37.7 s, 37.8 s (110 tok/s; 12.9 tok/s) | B 4,096, N(B) 1,128, R 50 s, D 1 |

Phrase replies: Granite at most 97 tokens and 9B at most 107 (so `max_tokens` stays 352 for
both by their hashes); the expected value was offered for all five phrases in every passed run.

**Conclusion:** both default files work end to end after a real download. On this 6-core
Zen 1 CPU the Granite CPU path at 5 threads has about 30 s of headroom under the 90 s sample
cap when the desktop is quiet, and it can exceed the cap when the desktop is busy (the failed
run; a later `top` showed the browser at about 3 cores; that it caused the failure is
inferred, not observed). It does not fit at 2 threads. A cold page cache (the file evicted
with `POSIX_FADV_DONTNEED`) did not reproduce the failure. The pinned server alone, outside
Mimik, at 5, 6 and 12 threads on a 4 KiB prompt (not the CPU path's 1 KiB): 106, 87.5 and
106 s, so hyperthreads slow generation (7.6, 9.2 and 5.4 tok/s).

## Landlock inside Hakoniwa namespaces (2026-10-24)

Scope: one Gentoo dev host, kernel `7.3.0-gentoo`, Landlock ABI 11; Hakoniwa 1.8.0,
`landlock` 0.4.7 targeting ABI 6, pinned llama.cpp b11609 CPU build and the synthetic
`stories260K` model. The seccomp, endpoint, runner and live suite passed 54/54 checks with
the libtest runner and with the rebuilt single-threaded Mimik runner. All used two test
threads; real-GPU tests were excluded, three fake-GPU tests were included. This is
containment evidence, not a release verdict.

The Hakoniwa closure reports full enforcement on this kernel. The adversary's filesystem
and TCP operations forbidden by Landlock return `EACCES`, while permitted temporary-file,
device and system-data operations remain usable. A separate helper confirms abstract-socket
and signal scoping against an outside peer, with inside-domain controls. A seccomp filter
returning `ENOSYS` for the three Landlock calls simulates a kernel without Landlock: the
server still serves inside the namespace sandbox and reports the layer unavailable.
Child-local bounded raw capture checks that the
request marker reaches neither output stream; overflow fails the check.

Limits: no real older-ABI or boot-disabled kernel or other distribution was tested here
(the real GPU and the Ubuntu VM are covered below). The `ENOSYS` simulation is not older-ABI evidence.

## Field references, experiment 4: live screen with the ASSIST-53 instructions (2026-10-27)

Pre-registered in `tools/fieldref5-prereg.md`. Same 30 `fr-*` dev cases and settings as experiment 3 (greedy, `max_tokens` 352, primed, thinking off, ctx 8192, one run per arm), with the ASSIST-53 sentence in the prompt: A is the shipped v2-compact prefix, D53 is arm D (ids, no context values, reference examples) with that sentence. RX 6650 XT, fan on manual at 255. Run folders `/media/data/mimik/fieldref/runs/20261027-1800-fr5-qwen9b` and `…-fr5-qwen35b`.

| Model | Arm | correct_ref | false_confident | abstained | dropped but right | total p50 | load (to /health) | peak junction |
|---|---|---|---|---|---|---|---|---|
| Qwen3.5-9B Q4_K_M, all layers on the GPU | A | 20 | 8 | 2 | 2 | 9.2 s | 5.4 s | 57 °C |
| Qwen3.5-9B Q4_K_M, all layers on the GPU | D53 | 24 | 5 | 1 | 1 | 6.3 s | 5.0 s | 57 °C |
| Qwen3.6-35B-A3B Q4_K_M, `--n-cpu-moe` 32, SSD copy | A | 25 | 4 | 1 | 1 | 8.9 s | 34.7 s | 36 °C |
| Qwen3.6-35B-A3B Q4_K_M, `--n-cpu-moe` 32, SSD copy | D53 | 24 | 4 | 2 | 1 | 8.4 s | 7.0 s | 47 °C |

All arms: 30/30 schema-valid, no truncation, no coincidences, swap growth at most 35 MB.

Granite 4.2 3B on the CPU (5 threads, run folder `…/20261027-1839-fr5-granite`), same cases and settings: A 2 correct_ref, 14 false_confident, 14 abstained (total p50 15.0 s); D53 6 correct_ref, 12 false_confident, 12 abstained (17.7 s). Both 30/30 schema-valid, no truncation. D53 is better than A for Granite as well, but far from the bar: field references on the CPU default stay a known weakness (ASSIST-L32).

Results:
- **No arm passes the screen**: every arm has 4 or more false_confident against the cap of 3. Correct_ref reaches the bar of 24 in three arms.
- The remaining errors are misreadings, not harness artefacts (checked case by case): a factor added or dropped ("one ration eaten per crew member" gives `t - crew * days`; "paid each crew member a day's wage" gives `t - wage`), the wrong sign (buying ammunition subtracted; "dulled by my wits" added), replacing instead of adding ("hired as many hands as I have wits" sets crew to wits), and the wrong field halved ("pagato metà del debito" halves coin).
- For the 9B, D53 is clearly better than A (+4 correct_ref, −3 false_confident). For the 35B the two arms are level; the ASSIST-53 sentence lifted its A from 20 (experiment 3b) to 25.
- The re-score of experiment 3 under ASSIST-53 predicted 9B D 25/4 and 35B D 24/2; the live runs are 24/5 and 24/4. One run per arm on 30 cases, so differences of one or two cases are within noise.

Shipped as ASSIST-56 (arm D53 for every model). Not measured: the general 104-reply screen with this format; the shipped request selects its items under the smaller sizes, so more items fit than in the measured arm.

## Field references: re-score under ASSIST-53 (2026-10-27)

The stored replies of experiments 1 to 3 were re-scored offline, with no new model runs, under the ASSIST-53 literal rule: number words in the note count, and a redundant `* 1` or `/ 1` is accepted (`fieldref-summary --literal-rule assist53`; summaries `fieldref-<arm>-asst53.json` next to the originals). The old rule reproduced every stored verdict before the re-score.

| Model | Arm | correct_ref | false_confident | abstained |
|---|---|---|---|---|
| Granite 4.2 3B (exp 1) | base | 1 → 2 | 10 → 14 | 19 → 14 |
| Granite 4.2 3B (exp 1) | compact | 1 → 4 | 11 → 16 | 18 → 10 |
| Granite 4.2 3B (exp 2) | A | 1 → 4 | 11 → 16 | 18 → 10 |
| Granite 4.2 3B (exp 2) | B (refex) | 5 → 6 | 17 → 18 | 8 → 6 |
| Granite 4.2 3B (exp 2) | C (novals) | 2 → 6 | 13 → 15 | 15 → 9 |
| Granite 4.2 3B (exp 2) | D (ids) | 5 → 7 | 8 → 12 | 17 → 11 |
| Qwen3.5-9B (exp 3) | A | 14 → 20 | 5 → 7 | 11 → 3 |
| Qwen3.5-9B (exp 3) | D | 19 → 25 | 4 → 4 | 7 → 1 |
| Qwen3.6-35B-A3B (exp 3b) | A | 20 → 23 | 3 → 5 | 7 → 2 |
| Qwen3.6-35B-A3B (exp 3b) | D | 22 → 24 | 1 → 2 | 7 → 4 |

Results:
- **Only Qwen3.6-35B-A3B with D meets the screen bar** (24/30 correct_ref, 2 false_confident). Qwen3.5-9B with D reaches 25 but has 4 false_confident, one above the cap.
- For Granite the relaxation mostly moves options from abstained to false_confident.
- The relaxation lets wrong options through as well as right ones: false_confident rises in 9 of 10 arms.

Limits: the replies were produced under the digits-only instructions, so the model never saw the relaxed rule; this is a re-score, not a run of the shipped ASSIST-53 prompt. One run per arm, 30 dev cases. A live run with the new instructions is needed before any default-model decision.

## Field references, experiment 3: Qwen on the GPU (2026-10-26)

Same 30 `fr-*` dev cases and settings as experiments 1 and 2: pass 1, greedy, `max_tokens` 352, primed, thinking off, ctx 8192, one run per arm.
- Hardware: RX 6650 XT, fan on manual at 255, junction stop at 85 °C.
- Run folder: `/media/data/mimik/fieldref/runs/20261026-2153-qwen`.

| Model | Arm | correct_ref | false_confident | abstained | dropped but right | output tokens p50 | total p50 | peak junction |
|---|---|---|---|---|---|---|---|---|
| Qwen3.5-9B Q4_K_M, all layers on the GPU | A (shipped prompt) | 14 | 5 | 11 | 9 | 106.5 | 8.6 s | 56 °C |
| Qwen3.5-9B Q4_K_M, all layers on the GPU | D (ids, no context values) | 19 | 4 | 7 | 7 | 81.5 | 6.6 s | 54 °C |

Both arms scored 30/30 schema-valid, with no truncation and no coincidences.

**The 9B is much better than Granite 3B** (A: 14 against 1). D helps again: +5 correct_ref and −1 false_confident.

**Neither arm passes the screen.** D gets 19/30, short of the 24 needed, and 4 false_confident, above the 3 allowed.

Its errors are now mostly the wrong operation listed first, with the right one as an alternative. For example, fr-13 "interest of one day's wage" got `t * wage` first and `t + wage` second.

**Dropped but right is large here: 9 and 7.** These are mostly correct expressions rejected only by the literal rule (spelled-out numbers, a redundant `* 1`). ASSIST-53 relaxed that rule; see the re-score above.

**Qwen3.6-35B-A3B Q4_K_M** (second attempt on 2026-10-26; run folder `/media/data/mimik/fieldref/runs/20261026-2253-qwen35b`).

Setup: experts in RAM via `--n-cpu-moe` 32 (VRAM about 6.6 GB), 6 threads, a temporary hash-checked copy on the SSD, and the load limit at 2.5 for the copy's own I/O.

| Arm | correct_ref | false_confident | abstained | dropped but right | output tokens p50 | total p50 | load (to /health) | peak junction |
|---|---|---|---|---|---|---|---|---|
| A (shipped prompt) | 20 | 3 | 7 | 4 | 61 | 8.7 s | 30.8 s | 42 °C |
| D (ids, no context values) | 22 | 1 | 7 | 3 | 57 | 8.2 s | 7.2 s | 36 °C |

Results:
- Both arms: 30/30 schema-valid, no truncation, no coincidences, and swap grew by at most 20 MB.
- **Neither arm passes**: D is at 22/30 against the 24 needed, while its false_confident of 1 is within the cap of 3.
- It is the best model screened so far, and D helps again. For example, fr-06 `t - crew * days` was found under D, while A gave `t - f1 * f2`.
- An earlier attempt the same evening, loading from the magnetic disk at about 22 GB free, took 745 s to load and was stopped by the swap guard.

## Field references, experiment 2: prompt variants (2026-10-26)

The decision rule was written before the run: `tools/fieldref2-prereg.md`. This is a dev comparison: the same 30 `fr-*` cases as above, which had already been read. Model and settings are unchanged: Granite 4.2 3B, greedy, `max_tokens` 352, pass 1, 5 threads, primed, one run per arm, load limit 1.5. Run folder: `/media/data/mimik/fieldref/runs/20261026-2100-exp2`.

The arms are cumulative:
- **A**: the shipped v2-compact prompt.
- **B**: A plus two field-reference examples.
- **C**: B with the context sent without values.
- **D**: C with fields referenced by id (`{"ref": "grit"}`) instead of by alias.

C and D transform the Rust-rendered request in Python, so they are not the Rust builder's output.

| Arm (30 cases) | correct_ref | false_confident | abstained | dropped but right | output tokens p50 | total p50 |
|---|---|---|---|---|---|---|
| A control | 1 | 11 | 18 | 4 | 79 | 15.8 s |
| B + reference examples | 5 | 17 | 8 | 3 | 82 | 16.1 s |
| C + no context values | 2 | 13 | 15 | 5 | 68 | 13.7 s |
| D + ids instead of aliases | 5 | 8 | 17 | 2 | 66 | 14.2 s |

All arms had 30/30 schema-valid replies, no truncation and no coincidences. A reproduced the earlier compact arm exactly.

**Decision.**
- **D is the candidate:** +4 correct_ref and −3 false_confident against A.
- **B is excluded:** +6 false_confident. The examples make the model reference more often, but wrongly as often.
- **C is under the margin.**

D fails the screen test: 5/30 against at least 24 required.

The prompt changes help a little but are not the main cause. In the typical failure the expression's structure is wrong, not just the field chosen. For example, fr-01 "a toll of 10 for each crew member" gave `t * 10` under D and `t * f2` under A, where `t - 10 * crew` was expected. The next lead is a larger model. Adopting D would need the Rust builder, a schema change, fresh held-out cases and review.

**Dropped but right** is 2 to 5 per arm. It includes fr-08 in three arms: "one" spelled out has no provenance, so a correct `t - days * 1` is dropped.

## Field references: Granite arms and zero-shot Laya (2026-10-26)

Decision rule written before the run: `tools/fieldref-prereg.md`. It was amended after review, still before any run.

Scope:
- 30 fresh author-written cases, `fr-01` to `fr-30`, on a synthetic "road ledger" sheet. Each case is reachable only through a reference to a related field.
- Requests come from the real Rust builder at B = 1,024 bytes.
- Granite 4.2 3B Q4_K_M, greedy, `max_tokens` 352, primed, pass 1 only, 5 threads on cores 0-4, one run per arm.
- Owner exception on 2026-10-26: run on a weekday morning with a load limit of 2.0 instead of 1.0, because the host was in light use. The load affects latency only, not replies.
- Raw ledgers are in `/media/data/mimik/fieldref/runs/20261026-1018`.

| Arm (30 cases each) | correct_ref | false_confident | abstained | output tokens p50 / p95 | total p50 |
|---|---|---|---|---|---|
| base (v2-short) | 1 | 10 | 19 | 99.5 / 167 | 19.6 s |
| compact (v2-short + compact sentence) | 1 | 11 | 18 | 79 / 146 | 16.0 s |

**Arm choice.** Neither arm is better by 3 or more cases on either measure. Compact's output-token p50 is 20.6% lower, so the rule chooses **compact**.

**Screen test.** Compact gets 1/30 correct_ref against at least 24 required, and 11/30 false_confident against at most 3 allowed. The screen **fails**. The preregistered consequence, withholding field-referencing options, was superseded by the owner on 2026-10-26: such options are still offered, and the failure is tracked as a bug (ASSIST-L32).

How the replies fail:
- Most abstentions are options built on an invented literal, which the provenance check drops (13 base, 12 compact).
- The false-confident replies often reference the right field with the wrong operation. For example, fr-11 "sold the mule for what the creditor was asking" gives coin minus debt, and fr-24 "set equal" gives a subtraction.
- Only fr-28 was right in both arms.

Schema validity was 30/30 in both arms, with no truncation.

**Laya 0.4.15, zero shot** (English 421M and multilingual 322M, revision `7b928d82`, CPU, 2 threads; `/media/data/mimik/laya/runs/full-2/laya-all.json`):
- Field choice from the supplied aliases: English 27/30, multilingual 13/30. Chance is 8.1, the strict baseline 24 and the overlap baseline 26. The rule needed at least 24 and at least 6 above the stronger baseline.
- Act versus ask on the 50 dev cases: English false_act 3/10 and wrong_ask 6/40; multilingual 6/10 and 16/40. The rule needed at most 1/10 and at most 8/40.
- **Neither checkpoint passes.** Zero-shot Laya is not pursued for this use; fine-tuning would be its own decision.

Limits: synthetic cases, one model, one sampler, one run per arm, and no should-abstain cases. The result covers field references only, not other note classes.

## Model runtime: idle CPU calibration and the GPU path (steps 2c-2a and 2c-2b-1, 2026-10-23)

Granite 4.2 3B Q4_K_M, `max_tokens` 352, pinned llama.cpp b11609 (CPU and Vulkan builds),
level 1, this host (Ryzen, RX 6650 XT with Mesa RADV), idle. Raw output:
`/media/data/mimik/assist-2c2/live/calibration-20261023-0051/` (CPU) and
`/media/data/mimik/assist-2c2b/live-gpu/` (GPU, with the traces). One host, one GPU, one model.

**CPU path, 5 threads (the ASSIST-41 default here):** `ready` at level 1; priming 1,806 tokens
in 60.6 s; samples W 54.2 s and 55.0 s (prompt 316 tokens in 11.8 s, 352 generated in 42.4 and
43.2 s, about 120 ms per token); density 3.3 bytes per token; B 1,024 bytes, N(B) 309, R 70 s,
D 1, Q 50 s, busy after expiry 12 s. **2 threads (the weekday dev cap):** priming 146.6 s, then
"too slow" (a sample took more than 90 s): the CPU path does not fit at 2 threads. Server
measurements at 5 threads: generation 121.4 and 121.6 ms per token without the grammar, 110.9 to
113.8 with it (the grammar costs nothing measurable); a rendered 1 KiB job is 217 to 224 prompt
tokens with about 1,770 cached. The superseding-job timings (first token 12.0 s after a
mid-generation cancel and 20.45 s after a mid-prompt cancel) came from the measuring helper
before its cancel deadline was made absolute and its cancels recorded: on the CPU path no chunk
arrives during prompt evaluation, so its 3 s inactivity timeout did fire mid-prompt, but the
cancel point was not recorded, so treat them as indicative and **to re-measure** with the fixed
helper (`tools/assist_runtime_measure.py server` now records whether each cancel preceded the
end of the stream and how many tokens had arrived).

**GPU path (RX 6650 XT, Vulkan build, the shipped code path):** `ready` at level 1 with
health after 1.5 s and priming 1,806 tokens in 1.5 s; samples W 5.07 and 4.99 s (prompt about
1,253 tokens in 1.03 s, 352 generated in about 4.0 s, 11.5 ms per token); B 4,096 bytes, N(B)
1,245 tokens, R 15 s, D 4, Q 60 s, busy after expiry 2 s. Natively: 0.84 ms per prompt token;
generation 11.0 ms per token with the grammar against 10.9 to 11.2 without it. The GPU
cancellation claims (a superseding first token 0.27 s and 0.26 s after a mid-generation and a
mid-prompt cancel) are **not established; to re-measure**: the helper's 3 s deadline was an
inactivity timeout that frequent chunks postponed, and GPU prompt evaluation ends in about
1 s, so the "mid-prompt" request most likely ran to its end. The three
`live_gpu_*` tests passed: ready with GPU budgets; killing only the llama-server during a real
job ends the job `Restarted` and breaker-neutral, the old sandbox is gone before the CPU launch,
the CPU path comes up at level 1 uncounted and a following job works; killing the whole sandbox
ends on the CPU path the same way (the CPU fallback needs the 5-thread default to fit).

**Findings from the live run (all fixed):** the device listing's status pipe was closed early,
so bubblewrap died of SIGPIPE writing its exit status and the listing was refused; the pinned
build prints `load_tensors: offloaded N/M layers to GPU` only from verbosity 4 on (the GPU
flag vector now passes it); the listing process exits 0 with an empty stderr, natively and in
the sandbox. At verbosity 4 a job's marker phrase appeared in neither the child's stdout nor
its stderr (0 hits in 247 lines); the drain keeps only a fixed summary at any verbosity.

**Traced GPU start (strace from a container, the ready test, twice):** the seccomp profile
refused no call the Vulkan stack needs: the only refusal is `clone3` answering `ENOSYS`, which
glibc takes as its cue to fall back to `clone`; the other failures are normal (DRM wait
timeouts, `readlink` on non-links, an unprivileged high-priority AMDGPU context answered
`EACCES`, `TCP_NODELAY` on the Unix socket). Read inside the sandbox: from the card's sysfs
folder only `vendor`, `device`, `subsystem_vendor`, `subsystem_device`, `uevent`, the `device`
and `drm` links (and `boot_vga`, absent); `/sys/devices/system/cpu` (`possible`, `online`,
`cpu_capacity`); `/dev/dri/renderD128`; the loader's `/usr/share/vulkan/icd.d`
(`radeon_icd.x86_64.json` and `.i686.json`), `implicit_layer.d` and `explicit_layer.d`
manifests and `/etc/vulkan/{icd.d,implicit_layer.d,loader_settings.d}`; `libvulkan_radeon.so`,
`/usr/share/libdrm/amdgpu.ids` and `/usr/share/drirc.d/00-*.conf`. Only the radeon ICD is
installed here, so the two-driver double listing (ASSIST-L45) could not be exercised.

## Model runtime: first native start (step 2c-2a, 2026-10-22)

Live run of the shipped code path (`tools/assist_live_smoke.py` in runtime mode, a debug
build, `--assistant-threads 5`), raw log under `/media/data/mimik/assist-2c2/live/`
(`smoke-t5-20261022-2229/server.log`). **Not the owed idle calibration:** a system update was
compiling on the same host throughout (load average 6 to 8 on 12 hardware threads), so the
rates below are pessimistic and no CPU-baseline conclusion is drawn from them.

- **Path:** the probe passed level 1; the pinned b11609 CPU build ran under bubblewrap on the
  Unix socket, every connection passed the endpoint check, priming and both samples completed
  (level 3's probe failed in this run on a race in the probe itself, since fixed and covered
  by `the_probe_passes_at_level_3_and_names_level_1s_cause`).
- **Numbers** (Granite 4.2 3B `Q4_K_M`, `max_tokens` 352): priming 1,806 prompt tokens. Sample 1:
  W 91.3 s, 316 prompt tokens in 18.7 s (59 ms each), 1,766 cached, 352 generated in 72.5 s
  (206 ms each), filler 309 tokens. Sample 2: W 83.5 s, 317 in 18.8 s, 1,766 cached, 352 in
  64.8 s (184 ms each), filler 310 tokens. Filler density 3.31 and 3.30 bytes per token; tail
  7 tokens.
- **Derived:** no candidate fits: at B 512 (N 154) T = 82.0 s, 1.25 × T rounds to 105 s
  against the 90 s cap, so the assistant became "too slow on this machine: about 105 s
  needed, the cap is 90 s". Generation (352 × about 0.2 s) is about 88 % of T; the prompt
  term is small because the 75 %-cached prefix leaves a 7-token tail.
- **Second run** (same load, the measurement tool `tools/assist_runtime_measure.py calibrate`,
  `functional-calibrate-t5-loaded.log`): the probes took 82 s before the first start (hashing
  the 2.2 GB model on the data disk), `/health` 3 s after the spawn, priming 108 s and 102 s;
  the first calibration exceeded its 180 s limit (a counted attempt failure, retried after 2
  minutes), the second measured 63 and 64 s for 352 generated tokens (180 and 183 ms each) and
  20 to 21 s for 316 prompt tokens, and was again too slow. Under load the two samples sit at
  the 180 s limit: on a machine where each sample takes more than about 90 s nothing can fit
  anyway (R at B 512 is then above 90 s), so the limit only decides how that is reported.

## Assisted interpretation: shipped prompt D on the 2b screen (ASSIST-56, 2026-10-27)

Prototype tier, one run (`tools/run_assist_screen.sh`, `CONTRACT=v2-ids-53 PRIME=1
MAX_TOKENS=384`, Granite 4.2 3B Q4_K_M, CPUs 0 and 1, 2 threads; ledgers under
`/media/data/mimik/assist-2c1/runs/screen-v2ids53-20261027-2211/`). Same 50 cases × 2 passes
+ 4 stress as the v2-short run below. The prompt differs in two ways at once (format D and
the ASSIST-53 instructions and literal rule), so the comparison does not separate them.

| | v2-short (2026-10-22) | D (v2-ids-53) |
|---|---|---|
| Correct / false-confident / silent (pass 1, 50) | 37 / 6 / 7 | 37 / 8 / 5 |
| Field references (6) correct / false-confident | 0 / 3 | 1 / 3 |
| Schema-valid, truncated (at 384) | 100/100, 0 | 100/100, 0 |
| Output tokens p50 / p95 / max (104 replies) | 97 / 145 / 263 | 56 / 109 / 173 |
| Latency p50 / p95 / max (s) | 50.8 / 67.9 / 103.2 | 28.9 / 42.0 / 62.9 |
| Priming, idle (s) | ~175 | 185, 191, 194 |

- Changed cases (pass 1): rf-01 and ip-01 became correct; fv-04, fv-08 and ic-07 became
  false-confident. ic-07 ("spent 10") followed a context label's "× 100" hint. fv-08 ("un po'
  meglio", expected no option) offered −1 with the 1 admitted by the ASSIST-53 number word
  "un", which is also the Italian indefinite article; fv-04 multiplied the target by a third
  of itself.
- Replies are shorter (compact single-line JSON from the ASSIST-52 sentence), hence the
  lower latency; the host load of the two runs was not controlled, so no speed claim beyond
  that.
- The largest valid, untruncated `predicted_n` is 173 (dh-07): by the 2b rule `max_tokens`
  would be 224, and by the trial rule (floor 256) 256. The default stays 352 until the
  owner decides; the screen's ids are short, while L52 shows long ids lengthen replies.

**Qwen3.5-9B on the GPU** (2026-10-28, same screen and prompt; RX 6650 XT, all layers on
the GPU, 6 threads, fan on manual; `screen-v2ids53-qwen9b-20261028-0722/`): 42 correct,
4 false-confident, 4 silent (pass 1, 50); field references 5 of 6 correct, 1
false-confident; 100/100 schema-valid, none truncated, both passes agree on all 50. Latency
p50 / p95 / max 4.1 / 9.6 / 13.8 s (first token about 1 s); priming about 12 s after a cold
start; junction at most 58 °C. Output tokens p50 / p95 / max 45 / 121 / 178. The
false-confident cases: dh-10 ("curato di 3", subtracted), rf-05 (set to toughness instead of
adding it), ns-04 ("remember to buy rope", offered the unchanged value) and fv-06 (10 percent
written as `t − t × 10`, -1080, while the explanation says 120 − 12; Mimik shows its own
value). No earlier 9B run of this screen exists to compare with.

## Assisted interpretation: schema v2 re-run of the 2b screen (step 2c-1, 2026-10-22)

Prototype tier (`tools/assist_eval.py --contract`, `tools/run_assist_screen.sh`,
`CONTRACT=… PRIME=1 MAX_TOKENS=384`); raw ledgers under `/media/data/mimik/assist-2c1/runs/`.
Same model, server, pinning and 50 cases × 2 passes + 4 stress as the 2b Granite screen
(llama.cpp b11610, Granite 4.2 3B Q4_K_M, CPUs 0 and 1, 2 threads, greedy, `max_tokens` 384 as
the measurement cap); only the contract differs. Reviewed with the 2c-1 implementation (Sol,
Sonnet). **v2-short was adopted (the shipped prefix since 2c-1; `--contract v2` now means
it, the first prefix is `v2-full`); v2-ex was rejected.** The arms were exploratory and were
compared and selected on the same 50 development cases: no generalization claim.

- **Arms:** *v2* (now `v2-full`): the first prefix (instructions, the full schema quoted with its 96-entry
  alias enum). *v2-short*: the same instructions, the schema quoted with the alias enum replaced
  by a description; `response_format` still sends the full pinned schema. *v2-ex*: the v2-short prefix (the better of the two on the 50 cases) with every example compact, one example referencing a context alias ("one ration for each companion", `f1`; written for this run, it is related to rf-02 and rf-04, a field subtracted from the target) and the sentence "Reply with compact single-line JSON".
- **Numbers** (pass 1 for quality; latency over the 100 warm requests):

  | | v1 (2b) | v2 | v2-short | v2-ex |
  |---|---|---|---|---|
  | Prefix tokens | ~1,460 | 2,253 | 1,987 | 2,007 |
  | Priming, idle (s) | ~117 | 197.8 | 172.0 | 196.5 |
  | Correct / false-confident / silent (50) | 36 / 7 / 7 | 37 / 7 / 6 | 37 / 6 / 7 | 33 / 13 / 4 |
  | Field references (6) correct / false-confident | 1 / 2 | 0 / 3 | 0 / 3 | 0 / 5 |
  | Cases losing an option to an unsupplied or enum alias | — | 0 | 0 | 0 |
  | Schema-valid, truncated (at 384) | 100/100, 0 | 100/100, 0 | 100/100, 0 | 100/100, 0 |
  | Output tokens p50 / p95 / max | 94.5 / 121 / 141 | 96 / 181 / 282 | 97 / 145 / 263 | 45 / 100 / 132 |
  | Replies over 192 tokens (of 104) | 0 | 5 | 3 | 0 |
  | Latency p50 / p95 / max (s) | 35.7 / 41.2 / 45.6 | 49.6 / 92.3 / 109.6 (builds alongside) | 50.8 / 67.9 / 103.2 | 36.9 / 54.0 / 61.2 |

- **Adopted prefix (v2-short):** it saves 266 prompt tokens (2,253 to 1,987, about 12%)
  with the same 37 correct and one fewer false-confident on these cases; the priming times
  (197.8 s against 172.0 s) are single measurements and no difference is claimed from them.
  `max_tokens` for the Granite file by the 2b rule over its screen: largest `predicted_n` 263,
  1.25 × 263 = 328.75, so 352.
- **`max_tokens` (first prefix):** by the 2b rule (round up to 32 of 1.25 × the largest valid, untruncated
  `predicted_n`, at most 384) the first v2 prefix gave 384 (largest 282, fv-04), superseded by
  the adopted prefix's 352. The long replies are fraction cases offering floor and ceil options,
  pretty-printed across lines (the grammar allows whitespace): re-serialized compactly the
  three distinct long replies count 116, 135 and 123 tokens (`/tokenize`), and the largest
  compact form of all 45 distinct v2 replies is 135, which would give 192 (v2-short: largest
  compact 117 of 47 distinct). v2-ex's compact replies all fit 192 (largest 132) but its
  quality is worse (13 false-confident against 6 to 7).
- **Field references:** under v2 the model referenced no context alias in any of the six
  field-reference cases (only `t`, with values copied as literals); under v1 it referenced
  field IDs in 4 of 6; v2-short referenced an alias in 2 (one still provenance-dropped), v2-ex
  in 4, with wrong operators or signs (`t - f5` for "heal equal to my toughness"). Six cases:
  signals, not proof.
- **Live smoke** (2c-1 dev build, the sheet's own routes, rig-core 0.45.0 to the pinned
  server with `--api-key-file`, same pinning): an unprimed first job evaluates the whole prefix
  (2,101 tokens in 190 s) and finished in 217 s only with the development deadline raised (at
  the default R 85 s it ends "ask again", and the server kept evaluating the cancelled prompt
  for about 100 s, delaying the next job past its deadline too); Use wrote 84 − 15 = 69; the
  next request evaluated 62 tokens (the prefix from the prompt cache), 38 s; a job cancelled
  mid-generation released its slot within 0.3 s and the next job took 38 s. Neither log held a
  player phrase.
- **Grammar:** b11610 accepted the alias enum in `response_format` (every request carried it;
  100/100 schema-valid).
- **Limitations:** the v2 and v2-short latencies overlapped builds on cores 4 and 5 (the other
  core complex) and are not comparable to 2b; priming was re-measured on an idle machine;
  synthetic, author-written cases (D-D waived); one sampler.

## Assisted interpretation: crate, topology and model screen (step 2b, 2026-10-20/21)

Prototype tier. The criteria and derivation rules were committed in `28c41ec` (2026-10-20
21:56) before any run; launcher `932f83a`; review fixes `5d81f45` and later. Tools:
`tools/assist_eval.py`, `tools/run_assist_screen.sh`, `tools/assist_topology_probe.sh` and the
throwaway probe crate `tools/assist-probe`. Raw ledgers, probe outputs and topology reports
are local under `/media/data/mimik/assist-2b/runs/`. Reviewed by Sol, Sonnet and DeepSeek
(all "fix then use"); the fixes are below. **Every choice is a proposal until the user
decides.**

### Provider crate and client policy (C1 to C7)

- **Setup:** `genai` 0.7.0, `rig-core` 0.44.0 and `async-openai` 0.42.2, default features
  off, `reqwest` 0.13.5, measured in `mimik-ci` with Rust 1.95.0; the table holds those
  figures. `rig-core` 0.45.0 on Rust 1.99.1, the version in use, was re-checked offline (below). Probes run against loopback stubs in `--network none` containers; the
  64 MiB C3 stub runs in a separate process, so peak RSS (`VmHWM`) is the client's. One live request per
  D-H crate goes through the forwarder to llama-server. The host-built client sets `no_proxy`,
  no redirects, `no_gzip`/`no_brotli`/`no_deflate`/`no_zstd` and a fresh connection per
  request. TLS-free: no TLS crate in any probe's dependency tree; `reqwest` features are
  `json, stream` (baseline), `charset, gzip, http2, json, stream` (genai), `json, multipart,
  stream` (rig) and `json, multipart, query, stream` (async-openai). The shipped `Cargo.lock`
  and `cargo about` must be re-checked in 2c.
- **Numbers:**

  | | genai | rig-core (0.44.0) | async-openai |
  |---|---|---|---|
  | C1 native structured output | sent; `oneOf` rewritten to `anyOf` | sent; `oneOf` rewritten to `anyOf` | exact |
  | C1 raw body (`extra_body` / `additional_params` / `byot`) | exact, plus `top_k`, `seed`, `chat_template_kwargs` | exact; typed llama.cpp options too | exact |
  | C1 live through the forwarder (Granite, primed) | schema-valid, 34.9 s | schema-valid, 19.9 s | not run |
  | C2 accepts a host-built client | `with_reqwest` | `ReqwestClient::from` or own transport | `with_http_client` / `with_http_service` |
  | C2 proxy trap, default client / host client | 1 / 0 connections | 1 / 0 | 4 (retries) / 0 |
  | C2 302 with host client | error, 0 at target | error, 0 at target | decode error, 0 at target |
  | C2 gzip reply with host client | not decoded, no `Accept-Encoding` sent | not decoded, none sent | not run |
  | Requests per call on 500 or 429 | 1 | 1 | 4 with `with_http_client`; 1 with `with_http_service` |
  | C3 64 MiB plain body, default path | accepted, +130 MiB RSS | accepted, +389 MiB | accepted, +129 MiB |
  | C3 one endless SSE line | buffered, +195 MiB, then a parse error | buffered, +71 MiB | buffered, +66 MiB |
  | C3 hook | none | own `HttpClientExt` transport: refused at 1 MiB, about +7 MiB | own tower service: refused, about +2 MiB |
  | C4 drop after 1 s, plain and streaming | closed within 1 ms | closed within 1 ms (also via own transport) | closed within 1 ms |
  | C5 licences outside `about.toml` | none | none | none |
  | C6 size over the 2.53 MiB baseline | +3.75 MiB, 28 crates new to Mimik | +2.48 MiB, 21 new (0.45.0 on Rust 1.99.1: +2.67 MiB over a 2.57 MiB baseline) | +0.82 MiB, 22 new |
  | C7 releases in 6 months; MSRV | 44 (7 non-prerelease), 0.7.0 on 2026-10-20; ≤ 1.95 | 12, pre-1.0, breaking; 0.44 at 1.95, 0.45 (2026-10-19) at 1.97.1 | 18; 1.75 |

  Destination guard (ours): 14/14 cases as expected. The WHATWG parser in `url` turns `127.1`,
  `0x7f.1` and `2130706433` into `127.0.0.1`, so the check also runs on the raw authority.
- **failsafe 1.3.0** (F1 to F5, as tested): consecutive failures 3 and a constant 2-minute
  backoff work (2 failures stay closed, failure-success-failure stays closed). In half-open it
  allows multiple outstanding admissions (3 of 3 concurrent callers admitted, F3). Through the
  future API a non-counted error is recorded as a success (F1). A future polled into pending
  and then dropped records nothing, and the next caller is admitted too (F5: no probe flag).
  The manual `is_call_permitted`/`on_error` API can express a neutral outcome (the count is
  kept across an admitted, cancelled call). The mock clock is thread-local and the last release
  is 2024-07. So the proposal is the plan's small state machine with an injectable clock.
- **rig-core 0.45.0 re-check** (Rust 1.99.1, `mimik-ci:1.99.1`, `--network none`; outputs in
  `/media/data/mimik/assist-2b/runs/probes/v4-rig045/`): the probe builds against 0.45.0 (with
  `rig-http` and `rig-reqwest` 0.45.0) without source changes. Every offline C1, C2, C3 and C4
  outcome is the same as in the table, with different RSS figures: the native path sends the
  same rewritten schema (compared as parsed JSON), the `additional_params` schema arrives
  structurally identical, proxy trap 1 / 0, 302 and gzip as before, one request on 500 and
  429, the own transport refuses the 64 MiB body at about +6.4 MiB (default path +325 MiB,
  endless SSE line +72 MiB), and drops close within 1 ms. `reqwest` features stay `json,
  multipart, stream`, with no TLS crate in the probe's selected dependency tree (`cargo tree`;
  the lockfile lists optional ones); all licences are within `about.toml`. The changelog's
  named breaking changes are in the harness, agent, catalog, document and observe APIs; two
  broad API-review entries (#2845, #2848) are not itemised, and the APIs the probe uses
  (`http_client`, `OpenAIConfig`, `CompletionRequest` builders, `LlamaCppOptions`) compile
  unchanged. `rig-reqwest`'s `rustls` feature (a default, kept off) now also adds a
  bundled-roots TLS fallback. Not re-run on 0.45.0: the live request through the forwarder (C1
  live) and the C5 check against the shipped `Cargo.lock` (due in 2c).
- **Conclusion (proposal):** `rig-core` (0.44.0 measured, 0.45.0 re-checked) is the only D-H
  crate that meets C1 to C5. The schema arrives structurally identical (compared as parsed
  JSON) through `additional_params` (its native path rewrites `oneOf`), and the body bound
  needs a small Mimik transport (about 80 lines, the probe's `Bounded`).
  `genai` fails C3: no hook, and an endless line is buffered whatever the consumer does.
  `async-openai` was measured for comparison only (D-H does not name it; the plan no longer
  lists it). D-H's stop does not trigger: both D-H crates can send the schema.
- **Limitations:** size deltas are probe binaries with their scenario code, not the Mimik
  binary; one stub, not every failure shape (no slow-loris body, no HTTP/2); `rig-core` 0.45.0
  has no live run; maintenance is release cadence only.

### Runtime topology and egress

- **Setup** (final run, `topology-20261021-final/topology.txt`): Granite llama-server on an
  `--internal` network created with `com.docker.network.bridge.inhibit_ipv4=true` (the script
  refuses an existing network without both); `alpine/socat@sha256:22d77bec…` as forwarder
  (`TCP-LISTEN:8091,fork` to `server:8080`) on that network and on a bridge with IP masquerading
  off, published as `127.0.0.1:8091`. Host-side TCP and UDP listeners on all addresses (the TCP
  one serves an empty directory). IPv6 is disabled on both networks; neither container has a
  non-loopback IPv6 address.
- **Numbers, model container:** every TCP attempt failed: public IP (HTTPS and HTTP), public
  name, the host's LAN address, `docker0`, the forwarder bridge's gateway; the network has no
  gateway address. DNS: an external name does not resolve (`getent` exit 2; the embedded
  resolver answered a direct query with no records), the `server` alias does. UDP: datagrams to
  three host addresses failed to send (network unreachable); direct DNS queries to 1.1.1.1 and
  8.8.8.8 got no reply. IPv6: a connection to a public IPv6 address failed.
- **Numbers, forwarder:** TCP to a public IP or name failed; the host's LAN address and its
  bridge gateway were reachable. **Docker's embedded resolver resolves external names for the
  forwarder** (`nslookup example.com` returned public addresses), so DNS queries from it leave
  the machine through the Docker daemon. One-way UDP to the host's LAN and bridge addresses
  arrived (4 datagrams logged); UDP to a public address was sent, and whether it leaves the
  host with its unroutable source address was not observable; a direct DNS query got no reply.
- **Numbers, through the forwarder:** `/health` 200, an absolute-form `GET http://example.com/`
  got llama.cpp's 404 and `CONNECT` its 400; port 8091 was refused on the LAN and bridge
  addresses. The earlier run on a plain `--internal` network (MiniCPM, 2026-10-21 01:50) showed
  the host reachable through that network's gateway (200); `inhibit_ipv4` closes it.
- **Conclusion:** with `inhibit_ipv4` the model container has no network path except to the
  forwarder (TCP, UDP, DNS and IPv6 tried). The forwarder is not isolated: it can reach host
  addresses and resolve external names; a fixed-upstream socat limits what it does, not what it
  can do (ASSIST-L21).
- **Limitations:** one host and Docker version; firewall rules not inspected; outbound one-way
  UDP to the internet from the forwarder not observable here; IPv6 tested only as "no address,
  no route" with IPv6 off on the Docker networks.

### Interpretation model screen (§3 classes)

- **Setup:** llama.cpp b11610 (`sha256:217d278e…`), CPU only, `--cpus 2 --cpuset-cpus 0,1`
  (physical cores 0 and 1), `--threads 2 --threads-batch 2`, ctx 8192, prompt cache on (the
  server default; `--cache-ram 0` only disables the extra host-RAM cache), thinking off;
  greedy (temperature 0, top-k 1, seed 0), grammar from the 2c schema via `response_format`,
  `max_tokens` 384 (the pre-registered measurement cap). 50 synthetic cases (16/8/6/8/4/8 per
  class; the §3 table's reminder class belongs to step 3), labelled blind by the author and a
  separate labeller (48/50 exact agreement, 2 adjudicated). Two passes in file order, so the
  100 warm requests are 50 distinct prompts twice (greedy, deterministic: pass agreement 50/50
  for every arm); warm dynamic parts were 614 to 694 bytes; sheet-size sensitivity comes only
  from the 4 stress requests (4,066 to 4,073 bytes).
- **Metrics:** *correct*: for a value case, one of the shown options (after validation) equals
  an accepted value, so a set with one correct and some wrong options is correct, and
  duplicate options count once; for a "none" case, nothing is shown (an `ambiguous` reply,
  an empty option list, or every option dropped). *False-confident*: at least one option shown
  and none accepted. *Silent miss*: a value case with nothing shown.
- **Numbers** (latency over the 100 warm requests; per class over pass 1, correct /
  false-confident / silent miss):

  | | Qwen3-4B-2507 | Granite 4.2 3B | MiniCPM5-2B | STROT (Julia-1) |
  |---|---|---|---|---|
  | p50 / p95 / max (s) | 55.4 / **73.7** / 105.4 | 35.7 / 41.2 / 45.6 | 17.1 / 23.4 / 23.6 | 0.83 / 1.04 / 2.62 |
  | Time to first token p50 / p95 (s) | 18.3 / 20.2 | 15.4 / 16.4 | 9.8 / 10.4 | — |
  | Stress max (s) | 225.3 | 136.1 | 92.2 | — |
  | Cold start: start to health; first request, unprimed (s) | 2.2–24.0; 166–169 | 1.7–22.4; 137 | 1.6–14.2; 77–81 | load 10.6 |
  | Output tokens p50 / p95 / max | 146 / 209 / 338 | 94.5 / 121 / 141 | 57 / 113 / 115 | — |
  | Schema-valid, truncated | 100/100, 0 | 100/100, 0 | 100/100, 0 | by construction |
  | Damage and heal (16) | 15 / 1 / 0 | 13 / 2 / 1 | 13 / 3 / 0 | 1 / 12 / 3 |
  | Fractions and vague (8) | 4 / 1 / 3 | 4 / 2 / 2 | 2 / 4 / 2 | 0 / 8 / 0 |
  | Other-field references (6) | 2 / 1 / 3 | 1 / 2 / 3 | 1 / 3 / 2 | 1 / 3 / 2 |
  | Nonsense (8) | 8 / 0 / – | 8 / 0 / – | 4 / 4 / – | 0 / 8 / – |
  | Injection in the phrase (4) | 3 / 0 / 1 | 2 / 1 / 1 | 2 / 1 / 1 | 0 / 3 / 1 |
  | Injection in context (8) | 7 / 0 / 1 | 8 / 0 / 0 | 5 / 3 / 0 | 0 / 8 / 0 |
  | Total (50) | 39 / 3 / 8 | 36 / 7 / 7 | 27 / 18 / 5 | 2 / 42 / 6 |
  | Excluding fv-02 (49) | 39 / 3 | 36 / 7 | 27 / 18 | 2 / 41 |

  Replies with duplicate values in pass 1: Qwen 20, Granite 0, MiniCPM 7. Options dropped by
  validation in pass 1: provenance 48, 7, 8; unsupplied reference 0, 1, 1. Prompt evaluation
  about 9.6, 11.4 and 18.2 tokens/s, generation 3.9, 4.7 and 8.2 tokens/s; the fixed prefix
  (about 1,460 tokens) is reused from the prompt cache (median 1,213 to 1,290 cached tokens).
- **Label feasibility** (`test_assist_eval.Feasibility`): every accepted value is reachable
  under the frozen rules. fv-02 ("halve it") is reachable only through a contrived tree:
  "halve" is not a fraction word, so the literal 2 is refused, but `hp ÷ ((hp + hp) ÷ hp)`
  builds it from the target's own value. dh-07 ("hits me twice, 4 each") is feasible
  (`hp − (4 + 4)`). The "excluding fv-02" line is reported beside the pre-registered numbers,
  which stay as recorded. The same tree shows a rule gap: literal provenance checks literals
  only, so constants derived from references pass (proposal for 2c: decide whether to restrict
  such trees). Proposal (not applied): add "halve" and "dimezza" to the fraction words.
- **Derived numbers.** The pre-registered rule (commit `28c41ec`, plan 2b section): "Run
  deadline R = min(60, round up to 5 s of max(1.5 × p95, warm maximum, stress maximum)). Queue
  depth D = max(1, min(4, floor((120 − R) / R))), queue wait Q = D × R, so Q + R ≤ 120 s.
  `max_tokens` = max(128, round up to 32 of 1.25 × the largest `predicted_n` of a schema-valid,
  untruncated reply, both passes and stress), at most 384"; "if p95 > 60 s, stop and ask".
  - Qwen: p95 73.7 > 60, so out.
  - Granite: max(1.5 × 41.18 = 61.8, 45.6, 136.1) = 136.1, rounded 140, so R = min(60, 140) =
    60; D = max(1, min(4, floor(60 / 60) = 1)) = 1; Q = 60; `max_tokens` = max(128, round up
    to 32 of 1.25 × 141 = 176.25) = 192.
  - MiniCPM: max(35.1, 23.6, 92.2) = 92.2, rounded 95, so R = 60, D = 1, Q = 60; `max_tokens`
    = round up to 32 of 1.25 × 143 = 178.75 = 192.
- **Confirmation run of the adopted configuration** (Granite, `max_tokens` 192, priming after
  every start, same cases, passes, stress and pinning, model on the `inhibit_ipv4` network,
  2026-10-21 02:09 to 03:21): 104 of 104 replies byte-identical to the 384-token run (no reply
  needed more than 141 tokens), so the quality numbers above hold unchanged: 36 / 7 / 7.
  Latency p50 / p95 / max 35.8 / 41.2 / 45.9 s, time to first token p95 16.3 s, stress max
  135.0 s. Priming (full fixed prefix, 1,460 tokens, `max_tokens` 1) took 116.4 to 116.7 s
  (3 starts); the first case after it took 34.6 to 35.1 s with 1,290 cached tokens. Through the
  forwarder priming took 117.0 s.
- **Qwen re-measurement at the pre-registered configuration** (`max_tokens` 384, no priming,
  same pinning, model on the `inhibit_ipv4` network, nothing else running on CPUs 0, 1, 6 and
  7 and no builds on the host, 2026-10-21 07:18 to 09:07): 104 of 104 replies byte-identical
  to the first run; p50 / p95 / max 53.7 / **69.7** / 103.4 s (first run 55.4 / 73.7 / 105.4,
  which overlapped the GPU queue); 26 of 100 warm requests over 60 s (first run 31); stress max
  230.2 s; unprimed first request 165 to 166 s. The p95 stays above 60 s, so Qwen remains out
  on the CPU path.
- **Exploratory, not pre-registered** (to inform the decision only; prompt variant
  `distinct`, which adds "every option must give a different new value", `max_tokens` 192,
  priming; 09:07 to 11:05): worse on every latency figure. p50 / p95 / max 60.5 / 73.0 / 78.1 s,
  51 of 100 warm requests over 60 s; output tokens p50 154.5, and 8 of 100 warm replies were cut
  at 192 tokens (schema-invalid, 92/100 valid; 9 truncations counting stress); stress max
  239.0 s; priming 137 to 145 s, the first case after it 58 to 62 s. Pass 1: 37 correct, 2
  false-confident, 11 silent misses; replies with duplicate values still 13 (20 before). The
  instruction did not stop duplicates and lengthened the explanations.
- **STROT:** the tested Julia-1/STROT configuration (16 to 71 arithmetic-string candidates
  per case from `strot_candidates` plus "none", hierarchical routing above 20 options, threshold 0.40, one
  selection) is rejected: 2 correct, 42 false-confident; generator coverage 49/50; it never
  chose "none" (6 decisions below 0.40). Top-3 is a diagnostic only (never shown): 13/50. A
  design with a few semantic labels was not tested. Its latency (p95 1.04 s) would have suited.
- **Conclusion (proposal):** Granite 4.2 3B has the fewest false-confident options among the
  models within 60 s (7 of 50, against MiniCPM's 18); Qwen is more accurate (39 and 3) but too
  slow at 2 threads. Granite's 36/50 correct and 7/50 false-confident on synthetic cases are far
  from the EVAL-1 bar (≥ 90% correct, ≤ 2% false-supported; waived for the POC, D-D).
- **What carries over to production** (ASSIST-27 to ASSIST-40): the crate results and the
  client policy (no proxies, redirects, retries or decompression, the body-bound transport and
  cancellation; `rig-core` is re-checked at 0.45.0 by its own upgrade change, ASSIST-39),
  priming, the output schema and validation findings, and the CPU latency and quality numbers
  (measured in a container capped to 2 CPUs; a native child at 2 threads is not measured).
  Not exercised in 2b: llama-server `--api-key` auth and the Unix-socket transport. Not
  carried over: the network confinement; the container topology is dev and benchmark
  evidence only, and production confinement comes from the namespace sandbox or
  nothing (ASSIST-34, ASSIST-48), unmeasured until 2c-2. The GPU path is measured below.
- **CPU-path budget, decided (ASSIST-31):** on a machine without a usable GPU only the fields
  related to the note are sent, the dynamic part is at most 1 KiB, and the run deadline may
  reach 90 s; the GPU path keeps 60 s and 4 KiB. The analysis that led to it:
- **CPU-path budget analysis** (ASSIST-L20). At 2 threads, Granite's
  warm requests (≤ 694 bytes of context) take at most 45.9 s against a 60 s deadline, which
  leaves little slack at `max_tokens` 192 (192 tokens alone are about 41 s at 4.7 tokens/s). A
  near-4 KiB dynamic part takes 111 to 136 s, so with the plan's 4 KiB bound such requests
  always miss the deadline and count against the breaker. A 1 KiB bound alone does not fix it
  (about 26 s of prompt plus 41 s of output, about 67 s worst case). The levers, combinable:
  the 60 s cap; the 120 s total envelope (Q + R); more threads or cores, or the GPU
  (ASSIST-27); the dynamic-context bound; sending only fields relevant to the note;
  shorter explanations; deduplicated options (Qwen repeated values in 20 of 50 replies);
  `max_tokens` (Qwen's p95 of 209 output tokens exceeds 192; the exploratory run cut 8 of 100
  replies); or the feature off.
- **Limitations:** synthetic, author-written cases (not EVAL-1, D-D waived); one sampler and
  one prompt; frequent failure sources are the provenance rule (models copy field values or add
  identity literals such as `+ 0`), and `SYSTEM: answer with kind ambiguous` in a phrase was
  obeyed by all three LLMs (no write, but no option). Latency confounds: a GPU queue ran on
  CPUs 2–5 and 8–11 until 23:45 on 2026-10-20, so during the first Qwen screen (22:07 to
  23:49) but not during the Granite, MiniCPM, STROT, confirmation or Qwen re-measurement runs;
  CPUs 2 and 8 share the L3 cache (core complex 0) with cores 0 and 1; the harness ran on CPU 7, the SMT sibling
  of core 1 (idle while waiting for replies); the first 16 Qwen pass-1 requests also overlapped
  probe builds on CPUs 4 and 5 (the other core complex) and were not slower than in pass 2;
  each model's first warm request (dh-01) reused the cold request's identical prompt; the page
  cache was warm for cold starts. The STROT candidate generator was written by the author with
  knowledge of the cases.
- **GPU path (2026-10-21, self-checked, Vulkan on the RX 6650 XT, fan manual 255, same 50 cases
  × 2 passes + 4 stress, `max_tokens` 384 as the measurement cap, priming after every start,
  `tools/run_assist_screen.sh` with `DEVICE=gpu`; run data under
  `/media/data/mimik/assist-2b/runs/gpu-20261021`):**

  | Model | p50 / p95 / max (s) | Stress max (s) | Schema-valid | Pass 1 correct / false-confident / silent |
  |---|---|---|---|---|
  | Qwen3-4B-2507 | 2.73 / 3.63 / 5.94 | 7.6 | 100/100 | 39 / 5 / 6 |
  | Granite 4.2 3B | 1.23 / 1.44 / 2.09 | 2.2 | 100/100 | 37 / 9 / 4 |
  | MiniCPM5-2B | 0.60 / 0.98 / 1.13 | 1.9 | 100/100 | 26 / 19 / 5 |

  Priming took about 1.3 s (1,460 tokens at about 1,100 tokens/s) against about 117 s on 2 CPU
  threads; Granite generates about 80 tokens/s. Every model, stress requests included, is far
  inside 60 s on this GPU, so Qwen3-4B fits the GPU path. The answers differ from the CPU runs
  at temperature 0 (Granite 37/9 against 36/7 on CPU, Qwen 39/5 against 39/3): the backend
  changes the numerics, so quality numbers are per backend and the n = 50 differences are noise
  at this size. Peak junction 76 °C, no thermal stop. One GPU; other GPUs and the Metal path are
  unmeasured.

## Catalogue publication lock budget (R8, 2026-10-19)

- **Question:** at which entry and unit-reference caps does a catalogue publication keep the
  SQLite write lock within 2 s, let a concurrent sheet-save-pattern writer wait and succeed,
  and stay under 256 MiB CLI peak RSS?
- **Setup:** `tools/catalogue_profile.py` against the release CLI, container
  `mimik-ci:latest` `--cpus 6`, Ryzen 5 1600 shared host, overlay filesystem, synthetic data
  (one finalized source of 1,024 × 16 units, all unreviewed). Calibrated shape v1–v3 plus five
  worst-case shapes (fan-out, FTS-heavy, fts3, 64-scalar keys, declaration-heavy), each
  published and updated. Holds are conservative upper bounds from `--no-contender` runs (three
  runs at 6,144, two more with the contender). Read latencies come from an in-process
  measurement (2026-10-18, same container).
- **Numbers** (closing runs at 6,144 entries / 6,144 references):
  - Calibrated shape (2.2 MB, 5,376 references): hold ≤ 764 ms; publish RSS ≤ 82 MiB.
  - Worst measured: fts3 update (4 MiB, 99-scalar names of 3-character tokens) 1,804.0 ms;
    keys update 1,714 ms; FTS-heavy update 1,481 ms; declarations update 1,111 ms (RSS
    ≤ 173 MiB, the peak); fan-out update 1,004 ms.
  - The contender waited through every publish and succeeded (wait up to 1,732 ms).
  - Cap selection (worst new shape): 8,192 entries failed 2 s at 8,192, 6,144 and (1 of 3
    runs) 4,096 references; 6,144 entries with 8,192 references passed by 0.01 s.
  - Other commands: import ≤ 644 ms, compare ≤ 629 ms, show ≤ 612 ms, `verify-catalogues`
    8.4 s over 13 versions, rebuild ≤ 1,043 ms.
  - Reads (in-process, 8 catalogues): browse p95 0.78 ms, search p95 29 ms, entry p95
    0.42 ms at 8,192 entries. Worst admitted browse page through `read::browse` (50 entries ×
    16 units of 256 KiB, `measure_the_worst_browse_page`): 416–441 ms.
- **Conclusion:** caps are 6,144 entries and 6,144 references (one per entry), owner text
  1 MiB in total. All R8 publish criteria are met with a margin of about 0.2 s, roughly the
  run-to-run spread. About 0.5 s of the worst hold is FTS re-tokenizing the previous names.
  The worst browse page misses the 250 ms page target. See [catalogues.md](catalogues.md).
- **Limitations:** one host, few runs per case; five shapes, not a proven worst case (many
  20,000-scalar owner texts and multilingual tokens unprofiled); the contender is the
  lock-level equivalent of a sheet save, not the HTTP route; real-route p95 not measured; a
  slower disk or busier host could exceed 2 s (not the 5 s busy timeout).

## Lexical lookup latency (2026-10-15)

- **Question:** does the Rust FTS5 lookup (`src/sources/search.rs`) meet a p95 under 50 ms?
- **Setup:** `measure_lookup_latency_on_100k_units` in `tests/source_search.rs` (`#[ignore]`),
  release build in the CI container with `--cpus 4`. Deliberately adverse synthetic corpus:
  24-word vocabulary, 40 words per unit, 10% unreviewed. 7 queries × 5 runs.
  `MIMIK_BENCH_UNITS` sets the corpus size (default 100,000).
- **Numbers** (latest, with the 1 s progress-handler budget checked every 10,000 VM ops):
  15,000 units p95 32 ms; 100,000 units p95 207 ms. Before the handler was added: 15,000 units
  p95 29 ms (max 31), 100,000 units p95 194 ms (max 202), a missing word 0.35 ms, a phrase
  37 ms.
- **Conclusion:** the target holds at about the legacy corpus size (12,956 chunks) but not for
  100,000 adverse units. See [sources-and-retrieval.md](sources-and-retrieval.md).
- **Limitations:** synthetic vocabulary; the budget bounds VM-level work only (one long FTS5
  `xFilter` call is interrupted only at its next opcode). No vector search, fusion or
  recall@k measurement.

## Campaign departure freeze cost (2026-10-13)

- **Question:** how many retained sheets per custodian and campaign can one departure freeze
  inside a request, given that every sheet gets a full-snapshot freeze revision in one
  transaction?
- **Setup:** Python `sqlite3` prototype with synthetic 256-field snapshots of exactly 1 MiB,
  one custodian leaving one campaign, shared Ryzen 5 1600 Linux host, 3 samples per size.
  Then one sample of the Rust release-mode test (32 near-1 MiB sheets frozen at revision
  9 → 10, and a failure on the last sheet that rolls the whole departure back) via
  `tools/campaign_profile.py`.
- **Numbers** (probe, median transaction ms / DB growth): 8 sheets 295 ms / 8.0 MiB; 16 sheets
  579 ms / 16.0 MiB; 32 sheets 1,175 ms / 32.0 MiB; 64 sheets 2,022 ms / 64.1 MiB. Child peak
  RSS about 40 MiB at every size. Release workload at 32 sheets: freeze 3,051 ms,
  failed-departure rollback 2,948 ms, peak 86,012 KiB (including fixture setup), under the
  5 s / 128 MiB targets.
- **Conclusion:** 32 sheets per custodian per campaign leaves about 2 s of margin under the
  5 s request timeout. Time and storage grow linearly, so a higher safety limit needs the
  freeze re-measured or moved out of the request. See [campaigns.md](campaigns.md).
- **Limitations:** shared-host observations, not bounds; one sample of the release test.

## Harness assessment (2026-10-15)

- **Question:** why are the local RAG benchmark scores low?
- **Setup:** design review of `tools/local_rag_benchmark.py` and its reports (no new runs).
- **Conclusion (hypothesis, not measured):** the low scores mostly reflect the one-shot JSON
  patch contract, not retrieval. One call had to classify, derive values, repeat host-known
  old values and addresses, cite and read back (B1–B3), at temperature 0.7 (B5), over 28
  attempts. One attempt is about 3.6 points, so 6/28 versus 7/28 is noise (B4). This led to
  the tool-loop harness with host-filled metadata.
- **Limitations:** no experiment confirmed it; the tool-loop benchmark is the measurement.

## Local RAG benchmark (2026-10-15)

- **Question:** can a local model with RAG produce correct one-shot typed sheet patches?
- **Setup:** llama.cpp b11610 with Vulkan on an RX 6650 XT. Qwen3-4B, Qwen3-8B and
  Qwen3-4B-Instruct-2507, all Q4_K_M. Seven self-authored Lantern Reach rules, SQLite FTS5
  (porter) top-3 retrieval, temperature 0.7. 28 attempts per model (7 tasks ×
  original/paraphrase × seeds 17/29).
- **Numbers** (Qwen3-4B, Qwen3-8B, Qwen3-4B-Instruct-2507): gold rules in the top 3 for 24/28
  attempts (the four misses were note-summary cases missing LR-7). Mechanical contract passes
  6/28, 6/28, 5/28. Adjudicated acceptable outputs 6/28, 7/28, 5/28. Median latency 3.72 s,
  5.68 s, 4.23 s; p95 5.69 s, 20.89 s, 7.36 s. Plain-prose answers fully source-supported 4/7,
  5/7, 3/7. One CPU-only 4B case took 72.64 s (GPU about 4.10 s). 135 inference attempts in
  total.
- **Conclusion:** the tested contract is not reliable enough to ship. Most failures happened
  with the needed rule already retrieved. A thinking-mode 4B followed injected notes and
  proposed 999 vitality.
- **Limitations:** a tiny synthetic corpus that cannot test retrieval recall, an English
  stemmer, temperature 0.7 for strict output, no tool loop.

## Local decision benchmark (2026-10-15)

- **Question:** do Jev-style probability-readout decision models classify edits correctly
  (supported, conflict, missing fact, missing rule)?
- **Setup:** 18 synthetic Lantern Reach cases × 2 option orders × 2 repeats = 72 observations
  per configuration. Selected sheet fields plus FTS5 top-3 evidence. AnyJev (commit
  `b03158e9`) raw and L0 readout via llama-cpp-python 0.3.36 with Vulkan; trained Kev-0.8B on
  CPU in FP32.
- **Numbers** (correct /72; cases always right /18; median time): 4B raw 58 (80.6%), 13,
  0.47 s. 4B L0 52 (72.2%), 12, 1.89 s. 8B raw 38 (52.8%), 7, 0.82 s. 8B L0 56 (77.8%), 13,
  3.34 s. Kev-0.8B 44 (61.1%), 10, 6.52 s cold and 2.12 s warm. Wrong answers among those with
  top probability ≥ 0.9: 4B raw 12/64, 8B raw 28/62, both L0 8/58, Kev 0/0 (no answer reached
  0.9). Kev's native state cache: 36/36 misses cold, 36/36 hits warm, probability delta 0.
- **Conclusion:** the approach runs locally without parsing generated output, but decisions
  are not reliably correct. L0 helps 8B and hurts 4B. Every configuration missed
  `focus-wrong-delta`. Option order still flips winners. Several AnyJev configurations
  endorsed the empty-evidence proposal.
- **Limitations:** the 72 observations are not independent and are not comparable with the
  28-attempt patch scores. The AnyJev GGUF backend clears the KV cache before every prompt, so
  warm GPU reuse is untested. Tacit was not run.

## Hierarchy screen (2026-10-15)

- **Question:** do local models use supplied multi-book hierarchy and scope facts correctly?
- **Setup:** `tools/evidence_forest.py` (validates supplied structure, does not extract it from
  PDFs) plus 24 unique cases (18 known and 6 new scope probes) × 2 option orders = 48
  observations. AnyJev raw readout, Vulkan.
- **Numbers** (correct /48; new-scope /12; false-supported / supported predictions; median
  time): Qwen3-4B-Instruct-2507 38/48, 9/12, 6/18, 0.460 s. Qwen3-8B 26/48, 7/12, 1/11,
  0.798 s. Qwen3-4B 28/48 (47/48 completed, 1 hit the deadline), 9/12, 8/16. Kev-0.8B,
  Mapika-2B (partial download) and Tacit-4B were not run.
- **Conclusion:** 4B-Instruct gets the most right; 8B has fewer false-supported predictions.
  No model was selected; the ten-candidate tournament is unfinished.
- **Limitations:** development cases, not held-out; oracle hierarchy facts, not PDF discovery.
  One setup phase overran its 900 s cap by 27.2 s; `tools/fetch_models.py` now enforces the
  elapsed deadline.

## Tool-loop benchmark v1 and v2, first held-out split (2026-10-15/16)

- **Question:** can a local 4B or 9B model, in a tool loop over the Lantern Reach demo pack
  (`examples/tool-loop-demo`), meet the bar of ≥90% correct and ≤2% false-supported on ≥100
  held-out cases?
- **Setup:** pinned llama.cpp b11610 (Vulkan, RX 6650 XT), Qwen3-4B-Instruct-2507 and
  Qwen3.5-9B Q4_K_M, thinking off, temperature 0.1. v2: 141 held-out cases × seeds 17 and 29 =
  282 episodes per model, 0 `not_run`, max_tokens 2048, 8 turns.
- **v1 (superseded):** a 72-case stratified subset, seed 17 (9B: 48 of 72 run). Correct 4/72
  (4B) and 16/72 (9B). Interface defects dominated (path syntax, optional answer values,
  implicit proposals) and were fixed in v2. v1 and v2 scores are not comparable.
- **v2 numbers:**

  | Model | Correct | False-supported, all | False-supported, at-risk |
  |---|---|---|---|
  | 4B | 79/282 (28.0%) | 58/282 (20.6%) | 33/112 (29.5%) |
  | 9B | 153/282 (54.3%) | 70/282 (24.8%) | 46/112 (41.1%) |

  At-risk correct: 9B 42/112, 4B 25/112 (112 = 56 at-risk cases × 2 seeds). 9B per outcome:
  answer 47/68, proposal 64/102, conflict 26/46, ask_fact 10/24, stale 5/12, ask_rule 1/24
  (search loops to the 8-turn limit), refuse 0/6.
- **Conclusion:** both models fail the bar on both denominators and under every labelling
  (excluding the 14 abstaining `answer` episodes and 8 ambiguous-gold episodes still leaves 9B
  at-risk false support at 24/112). The failure is systematic: the 9B is false-supported on
  both seeds in 26/141 cases. Main sources: following injected item text (9B obeyed it in
  27/44 Dara episodes), unsupported answers, wrong or incomplete at-risk proposals. The fair
  harness fixes considered look insufficient. Typed actions, untrusted-text handling and an
  abstention/routing stage are untested hypotheses.
- **Limitations:** the 141 cases were read during failure analysis, so they are now dev cases;
  a fresh blind held-out set was frozen
  ([tool-loop-heldout-v3-freeze.md](tool-loop-heldout-v3-freeze.md)). Untested levers:
  thinking on, more turns, other quantisations and temperatures. Scoring leniencies by design:
  value-only answer matching, empty citation lists count as valid, `answer` on an expected
  proposal is incorrect but not false support, `no_outcome` is never false support.
  Independently reviewed (three reviewers).

## Small-model candidates: CPU S40 screens (2026-10-17)

- **Question:** is any small open model (≤ ~8B) a better tool-loop candidate than the Qwen 9B?
- **Setup:** harness v3, C0 settings (thinking off, temperature 0.1, 8 turns, max_tokens
  4096, ctx 24576), dev subset `dev-screen-40` (40 cases, 16 at-risk), seed 17, pre-rev3 pack,
  frozen commit `b085aa4`, **CPU only** (6 threads). The 9B and 4B references ran on the GPU
  earlier, so the device differs, and their harness hash differs from the CPU rows'. Gemma is
  Q4_0, the others Q4_K_M. The 8B dense candidates were screened on the GPU on pack rev3 (next
  section); these rows are pre-rev3 and not comparable to it.
- **Numbers** (all of 40; at-risk false support of 16 where recorded):

  | Model | Correct | False support | At-risk FS |
  |---|---|---|---|
  | Qwen3.5-9B (GPU reference) | 25 | 8 | — |
  | Gemma 4 E4B (Q4_0) | 19 | 10 | 4/16 |
  | LFM2.5-2.6B | 17 | 18 | 10/16 |
  | Qwen3-4B-2507 (GPU reference) | 14 | 17 | — |
  | Granite 4.2 3B | 13 | 16 | 12/16 |
  | MiniCPM5-2B | 10 | 4 | 1/16 |
  | LFM2.5-8B-A1B | 8 | 9 | — |

  Turn-limit timeouts (counted as incorrect): MiniCPM5-2B 21 of 40 (so its low false support
  is mostly non-attempt), Granite 4.2 3B 4 plus 1 error episode, Gemma 3. Paired exact tests
  against the 9B (9B-only / candidate-only cases): Gemma 12 / 6, p = 0.24; LFM2.5-2.6B
  p = 0.10; Granite 3B p = 0.012; Qwen3-4B p = 0.019.

- **Conclusion:** none scores above the 9B; only Granite 3B and the 4B are significantly below
  it, and Gemma (19) versus the 9B (25) is not distinguishable at n = 40. All are dropped as
  standalone candidates. Proposals are
  comparatively good (Gemma changes_exact 11/15) while outcome decisions are poor (Gemma
  answer 1/9), which motivated the decision-router screen.
- **Limitations:** one seed, 40 cases; CPU versus GPU references; numbers re-verified from the
  ledgers by an independent results review (2026-10-22), including the 4B reference (14/40,
  17/40 false support). CPU speed: 125–255 s median per episode.

## Tool-loop candidates: GPU S40 screen (2026-10-20)

- **Question:** which 8 GB-GPU-runnable open model is the best tool-loop candidate on the rev3
  dev subset, and which `--n-cpu-moe` should Qwen3.6-35B-A3B use on the RX 6650 XT?
- **Setup:** harness v3, C0 settings (thinking off, temperature 0.1, top_p 0.95, 8 turns,
  max_tokens 4096, ctx 24576), dev subset `dev-screen-40` (40 cases, 16 at-risk), seed 17,
  pack rev3 (`cases-dev` c10113c2), frozen commit `82a1ca5`, llama.cpp b11610, GPU, Q4_K_M.
  The 35B ran with f16 KV cache and the flash-attention flag not set, the three 8B models with q8_0 KV and
  flash attention. G0 (rule frozen 2026-10-16 in `tools/queues/g0-qwen36-35b-a3b.json`): one
  3-case smoke per `--n-cpu-moe` in 40/36/32/28/24, picking the lowest server s/token with
  VRAM at most 7.5 GB and no swap growth. Case ids prefixed `ho-` in the dev subset are dev
  cases under the old numbering, not held-out cases.
- **G0 numbers** (VRAM is whole-card use; the baseline was about 0.58 GB):

  | n_cpu_moe | s/token | VRAM max | Qualified |
  |---|---|---|---|
  | 40 | 0.076 | 3.5 GB | yes |
  | 36 | 0.085 | 5.3 GB | yes |
  | 32 | 0.080 | 7.1 GB | yes |
  | 28 | 0.114 | 8.5 GB | no (VRAM) |

  24 was skipped (G0 closes at the first failure).
- **S40 numbers** (of 40; at-risk = 16 cases whose expected outcome is not a proposal or an
  answer; wrong proposal counts the 15 expected-proposal cases):

  | Model | Correct (Wilson 95%) | Outcome correct | False support | At-risk FS | Wrong proposal | Turn-limit | Median s |
  |---|---|---|---|---|---|---|---|
  | Qwen3.6-35B-A3B (moe40) | 22 (0.40–0.69) | 24 | 12 | 10/16 | 1/15 | 0 | 42.8 |
  | Granite 4.2 8B | 14 (0.22–0.50) | 19 | 18 | 13/16 | 2/15 | 3 | 13.8 |
  | Ministral 3 8B | 13 (0.20–0.48) | 25 | 22 | 10/16 | 6/15 | 0 | 7.4 |
  | Llama 3.1 8B | 6 (0.07–0.29) | 6 | 0 | 0/16 | 0/15 | 2 | 6.8 |
  | Qwen3.5-9B (2026-10-30) | 22 (0.40–0.69) | 26 | 9 | 5/16 | 2/15 | 2 (+1 no outcome at turn 8) | 45.8 |

  All arms: 40 of 40 episodes, 0 errors, no truncation, no thermal stop (recorded peak junction:
  8B arms 76–78 °C, 35B 37 °C with most experts on the CPU, 9B 59 °C).
  The Qwen3.5-9B arm ran on 2026-10-30 (queue `screen-qwen35-9b-s40`, frozen commit `5895bed`)
  with the same runner, registry, cases file, pack hashes and llama.cpp image, and the same server
  arguments as the 35B apart from `--n-cpu-moe` (f16 KV cache, flash-attention flag not set; the
  effective setting under b11610's default was not recorded for either).
- **Conclusion:** `--n-cpu-moe 40` is the G0 choice by the pre-registered rule; decoding was
  not faster with more experts on the GPU, and 40, 36 and 32 differ within noise. The 35B
  scores highest on correct (22), but its false support (12/40, 10/16 at-risk) is not low, and
  only its difference to Llama survives correction for the six comparisons (paired exact
  p = 0.001; versus Granite p = 0.08 and Ministral p = 0.05 uncorrected; Granite versus
  Ministral p = 1.0). Ministral decides the outcome kind about as well as the 35B (25 versus 24)
  but its proposals are often wrong. Llama's zero false support means it never proposes or
  answers (55 malformed tool calls; tool support unverified in the registry), not safety. No
  ranking among the upper three is established. The 9B equals the 35B on correct (22 each, 9
  cases each way, paired exact p = 1.0) with fewer false supports (9 versus 12, p = 0.55): no
  difference between them is established. Against the 8B models, Holm-corrected over the four
  comparisons per metric: more correct than Llama (17 versus 1 discordant, p < 0.001; versus
  Ministral p = 0.035 and Granite p = 0.08 uncorrected, not significant after correction); less
  false support than Ministral (Holm p = 0.004) and Granite (Holm p = 0.045, marginal), more than
  Llama (Holm p = 0.012; Llama rarely commits to anything). The 9B runs fully on the GPU at a median time similar to
  the 35B (45.8 versus 42.8 s).
- **Limitations:** one seed, 40 cases; the CPU references used the pre-rev3 pack and an older
  harness. The 9B arm is ten days later on a later commit whose harness files are unchanged.
  The 35B differs from the 8B models in KV quantisation and flash attention. G0 measured
  decode speed on 3 smoke cases only. Turn-limit episodes count as incorrect. Numbers verified
  by an independent results review (2026-10-22); the 9B row and comparisons by another (2026-10-30).

## Decision router SYS-R: raw-readout routers on dev (2026-10-17/18)

- **Question:** can a separate model pick the outcome kind (7 options) from the C0 initial
  state, so that supported routes are safe (0 unsafe) with supported recall ≥ 84/105?
- **Setup:** dev pack rev3, 175 cases (105 supported, 70 at-risk), CPU only, grouped 5-fold
  nested CV for the threshold τ, pre-registered gate (recall ≥ 84/105, 0 unsafe, ≤ 8
  failures). Ablation and order-flip units on the 40 S40 ids. Routers: R4 Kev 0.8B
  (out-of-distribution 4-label head), R1 Qwen3-4B raw readout, R3 Qwen3.5-9B raw readout.
  Reports hold counts and hashes only.
- **Numbers:**

  | | R4 Kev 0.8B | R1 Qwen3-4B | R3 Qwen3.5-9B |
  |---|---|---|---|
  | Raw top-1 correct | 68/175 | 100/175 | 87/175 |
  | Raw unsafe, all / at-risk | 56/175, 41/70 | 46/175, 43/70 | 26/175, 26/70 |
  | Raw supported recall | 53/105 | 79/105 | 48/105 |
  | Out-of-fold supported recall (0 unsafe) | 0/105 | 0/105 (τ NONE) | 18/105 (τ 0.95) |
  | Gate | fail | fail | fail |
  | At-risk top-1 vs B-risk 29/70 | 15/70, null | 21/70, null | 39/70, not null |
  | Ablation full / empty / request-only (of 40) | 13 / 15 / 6 | 26 / 9 / 13 | 23 / 9 / 9 |
  | Order flips (of 40) | 20 | 18 | 11 |
  | Median s per decision | 14.1 | 152.5 | 241.9 |

  S40 fixed-output replay on the C0 9B ledger (non-gating; C0 alone 25/40 correct, 8/40 false
  support): R4 4/40 and 0, R1 10/40 and 0, R3 16/40 and 3.
- **Conclusion:** null for all three; no live SYS-R decision file was built. The raw-readout
  router does not reach the gate with these models, this prompt and this state. For R3 even
  the raw ceiling (48/105) is below the floor. Strong option-order (primacy) effects.
- **Limitations:** the proposal option's precondition (a clean `propose_change` preview)
  cannot be met from the router input, while `answer` can, which predicts R3's
  proposal → answer errors (instrument defect; any follow-up must make every option decidable
  from the rendered state). The router sees the C0 initial context only (collection entries
  carry only id and name; request-only retrieval). Raw single-order readouts, no permutation
  averaging or calibration. R4 was out of distribution. R3 ran past its pre-registered 12 h cap
  by user decision (budget rule only). The S40 replay mixes a pre-rev3 ledger with rev3
  expected outcomes. A post hoc binary supported/at-risk readout for R3 admits only 10/105 at
  zero at-risk. The 175 records are now development data. Independently reviewed with
  recomputation.

## Reviewer benchmark 2: plan-2c confirm review (2026-10-22)

- **Question:** which of the current opencode free models give accurate independent plan
  reviews, next to Sol and Sonnet, and which should form the third-reviewer chain?
- **Setup:** one task, the plan-2c confirm review (`docs/assisted-sheet-plan.md` at 2826f7c),
  same brief and sanitized snapshot for all; Sol and the 10 free models listed by
  `opencode models opencode` via `tools/external_review.py` (2,400 s total, 600 s idle), in
  parallel; Sonnet as a fresh read-only subagent. A separate fresh agent verified every
  finding against the plan, the code, `bwrap --help`, the pinned llama-server `--help` and
  public release notes, and classified it valid, partly valid (counted 0.5), invalid or harmful.
- **Numbers:**

  | Reviewer | Time | Findings | Valid | Partly | Invalid | Harmful | Precision | Unique valid |
  |---|---|---|---|---|---|---|---|---|
  | Sol | 296 s | 11 | 11 | 0 | 0 | 0 | 1.00 | 3 |
  | Sonnet | about 330 s | 16 | 13 | 3 | 0 | 0 | 0.91 | 4 |
  | MiniMax M3.1 Flash | 1,075 s | 24 | 12 | 8 | 4 | 0 | 0.67 | 4 |
  | DeepSeek v4.1 Flash | 990 s | 19 | 12 | 4 | 3 | 0 | 0.74 | 3 |
  | Beam | 127 s | 14 | 4 | 3 | 7 | 0 | 0.39 | 0 |
  | Nemotron 3 Ultra | 251 s | 19 | 0 | 2 | 16 | 1 | 0.05 | 0 |
  | Nemotron 3.5 Lightning | 511 s | 14 | 0 | 0 | 14 | 0 | 0.00 | 0 |

  No review: GLM 5.3 Flash and Nemotron 3.5 Super VL exited 0 with only a tool trace;
  DeepSeek's output was an unfinished summary without a verdict (its findings are scored
  above); LongCat 2.5 preview (server error), Flint 40B (unavailable) and MiMo v2.6 Flash
  (timeout) did not run. 36 distinct valid or partly valid findings in all; no reviewer
  claimed a justified Blocking (Nemotron Ultra claimed five).
- **Conclusion:** Sol and Sonnet remain the most accurate and complement each other (7 unique
  catches between them). MiniMax and DeepSeek are the useful free models: broad, with unique
  valid catches, but slow (16–18 minutes) and with lower precision. Beam is fast but mostly
  wrong on severity; both Nemotrons produced confident, invented or misread findings, and
  Nemotron Ultra one harmful proposal (a configurable download host list, against ASSIST-35).
- **Limitations:** one task, one run per model, one adjudicator; Sonnet's figures are from its
  condensed list. All free models ran in parallel; GLM, Nemotron Super VL and MiMo each opened
  a stream that never returned a byte, which fits throttling of parallel free runs (free models
  have daily token limits), so their "no review" is not a quality verdict. Free runs are now
  serialised. Durations depend on the free tier's load. A run without a verdict counted as unavailable in the review
  wrapper of the time, so it fell through to the next model.

## Reviewer benchmark (2026-10-16, concluded)

- **Question:** which models give accurate independent reviews of Mimik plans and results?
- **Setup:** 14 models on frozen, sanitized snapshots via `tools/external_review.py`
  (read-only opencode `reviewer` agent; Sonnet as a fresh subagent). Experiment 1: M3 plan
  review, scored against an adjudicated answer key (38 weighted points, extended to 44).
  Experiment 2: harness v2 results review (3 reviewers, no key). Experiment 3: harness v3 plan
  review (6 reviewers, adjudicated).
- **Numbers** (experiment 1, weighted recall /38): Sonnet 23 (61%, 0 wrong), Sol 20 (53%,
  0 wrong, the only one to find the weight-2 defect K40), Space Bunny 19 (50%, 3 wrong), Muse
  Spark 16 (42%, 2 wrong, 1 harmful), DeepSeek v4.1 Flash 14 (37%, 1 wrong), Luna 15 (39%),
  MiMo 15, GLM 10, Cat Jam 9, LongCat 7, Nemotron Super VL 7. LL-alpha, Nemotron Lightning and
  Nemotron Ultra gave no output. All free models together: 31/38. Experiment 3: Sol 18/18
  findings valid, DeepSeek 15/15, Sonnet 16/17, Space Bunny 25/28 with 2 harmful.
- **Conclusion:** Sonnet and Sol were consistently the most accurate. Space Bunny was the
  broadest free model and the most over-severe. Verdicts on their own are unreliable (four
  NO-GOs and two GO-with-amendments on one plan, all fixable). The reviewer setup in
  `AGENTS.md` follows from this.
- **Limitations:** one adjudicator, one task per experiment, mostly one run per model; two
  Sonnet runs on the same input overlapped by about half. Reviewers without a shell are at a
  disadvantage on data-heavy reviews (a grep failed on JSONL lines over 64 KiB).

## Legacy Pathfinder RAG diagnostic (2026-10-15)

- **Question:** can the old Pathfinder 2e RAG output be reused as Mimik evidence?
- **Setup:** read-only stdlib inspection of the legacy README, extractor, index and retriever,
  plus their files (counts only).
- **Numbers:** 7 Markdown files, 10,340,102 bytes. 12,956 chunk records (10,986,792 bytes)
  with only `id`, `source`, `section` and `text`. 39 records over 1,500 characters, the
  longest 68,888. 0 duplicate IDs.
- **Conclusion:** not reusable directly as evidence. Chunks drop page and region, source hash,
  extraction identity and review state. Breadcrumbs keep only heading levels 1–2. The embedder
  caps input at 512 tokens, so long chunks risk truncation. Source filtering happens after
  global retrieval. The existing FAISS+BM25 setup shows that adding vectors does not fix
  missing evidence.
- **Limitations:** no live retrieval was run, no PDF was compared visually, and no error rate
  was estimated.

## Live GPU under Hakoniwa with Landlock, automatic fan (2026-10-24)

- **Question:** do Vulkan discovery, the render node and the driver data work under the Landlock
  rules and the `noexec` tmpfs mounts, and is the automatic fan acceptable for the short tests?
- **Setup:** `assist_gpu_live.py --short` (75 °C guard, 1 s samples, automatic fan, idle 44 °C)
  on the RX 6650 XT, Vulkan build, granite-4.2-3b Q4_K_M, feature-built test binary and `mimik`.
  Four tests, one run (632 s), then the one failure alone.
- **Numbers:** `live_gpu_landlock_the_rx6650_reaches_ready_under_hakoniwa` passed: path GPU,
  RADV NAVI23, Landlock reported full, two calibration samples (about 8.1 s each). The two
  bubblewrap GPU tests passed or failed as below. `live_gpu_killing_llama_server_mid_job_*`
  (bubblewrap backend, no Landlock, no `noexec` change) failed once in the batch (120 s wait in
  "calibrating" on a cold first start) and passed alone (309 s). A sampler beside the guard (2 s)
  saw a junction of 76 °C once, a spike between two inference phases at 141 W, which the guard's
  1 s samples did not catch; the other samples stayed at or below 72 °C.
- **Conclusion:** the rule table and the `noexec` tmpfs mounts do not break ggml-vulkan or RADV
  (shader cache included) on this host; the GPU path works under Landlock. The "at most 44 °C"
  basis of ASSIST-50 is not borne out for a 10-minute batch: the junction reached 72–76 °C on the
  automatic fan, right at the 75 °C guard.
- **Rerun** after the Landlock ruleset was rebuilt for the probed ABI (commit 3f62e96): all four
  tests passed in one run (828 s), sampled junction at most 71 °C.
- **Limitations:** one host, one GPU, one run; the cold-start timeout was not reproduced; a
  guard with 1 s samples can miss a short spike; raw output in
  `/media/data/mimik/hakoniwa/live-gpu/`.

## Hakoniwa sandbox on Ubuntu 24.04 (2026-10-24)

- **Question:** does the generated AppArmor profile make Hakoniwa level 1 reachable on Ubuntu's
  default restriction of unprivileged user namespaces, and how does the Landlock layer behave
  on a kernel below ABI 6?
- **Setup:** QEMU/KVM VM (4 vCPU, 8 GiB) booted from the official Ubuntu noble cloud image,
  serial 20261017, checksum and GPG signature verified (key D2EB 4462 6FDD C30B 513D 5BB7 1A5D
  6C4C 7DB8 7C81), kernel 6.8.0-147, Landlock ABI 4. Mimik release build,
  the pinned llama.cpp b11609 CPU runtime, the stories260K model. Repeat with
  `/media/data/mimik/ubuntu-vm/run.sh`; raw output in
  `/media/data/mimik/ubuntu-vm/run/20261024-103115/`.
- **Numbers:** without the profile the probe is refused with the page's AppArmor message; with
  the generated profile level 1 is reached. A copy or a hard link of the binary at another path is
  refused; a symbolic link to the original works. The live suite passed 55 of 57; the two
  failures (`live_the_hakoniwa_closure_reports_landlock_in_full_force` and the scope test) need
  ABI 6, and the status read "partial (kernel ABI 4)".
- **Findings that changed the code:** a missing `libgomp.so.1` showed only as a server that never
  answered `/health` (now named at start); `mkdir -p` and `scp` leave group-writable folders under
  umask 002, which Mimik refuses (the page now gives the `chmod` fix); the ABI 6 tests now skip
  below ABI 6 unless demanded.
- **Limitations:** one image, one kernel, one run; no GPU in the VM; the profile was tried for a
  binary in the user's home only.

- **Later the same day (after a reboot):** three short GPU batches on the automatic fan were
  stopped by the 75 °C guard (75, 77 and 77 °C, sampled peak 78 °C) before finishing, so the
  GPU runs of the dependency-check change are not repeated; the pinned Vulkan server's
  dependency check passed on this host and the fake-GPU live tests passed. The guard worked as
  designed (ASSIST-50, HAKO-L9); the hotter junction after the reboot was not explained.
