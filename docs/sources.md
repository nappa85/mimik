# Local source inventory

Inspected: 2026-10-11. Paths below are local inputs, not bundled project assets. No full manual extraction or ingestion has been performed for Mimik. The older Pathfinder experiment was inspected for source quality, not adopted as an implementation reference.

## Primary manuals

| Source ID | Local path | PDF pages | Identification and status |
| --- | --- | ---: | --- |
| `pf2-core-it-2022` | `~/temp/pathfinder/Manuale di Gioco.pdf` | 646 | Italian Pathfinder Seconda Edizione Core Rulebook, credits state September 2022 printing; legacy/pre-remaster source. Exact errata coverage not checked. File contains added front matter and must have its own page map. |
| `dnd5-phb-it-legacy` | `~/temp/D&D5 Manuale del Giocatore.pdf` | 321 | Italian Player's Handbook, legacy 2014-era rules indicated by contents and May 2014 preface; exact translation printing/errata not checked. Existing OCR text contains visible errors. |
| `vtm5-core-it-2019` | `~/temp/514123511-Vampiri-la-Masquerade-5-Edizione.pdf` | 432 | Vampire: The Masquerade V5, Italian NEED GAMES edition; credits state 2019 printing. Text extraction is permitted by the PDF permissions and succeeded; exact errata coverage not checked. |
| `mork-borg-core-it-2025` | `~/temp/MORK-BORG-ita.pdf` | 96 | Italian NEED GAMES first printing, credits state October 2025. Text layer available; complex layout and language/version differences require review. |
| `mork-borg-core-en-2019` | `~/temp/Mörk Borg.pdf` | 96 | English text, credits state 2019 printing. Text layer available; do not assume equivalence with the Italian source. |

Identification used PDF metadata and sampled front matter, not filename alone. These are candidate source identifiers, not implemented rules-adapter identifiers.

### SHA-256 identities

```text
pf2-core-it-2022         1be771e5c9b1e01260fd547b9e4f6358f2b7c84e2adc75f3e3805be7d4199901
dnd5-phb-it-legacy       84a1967b032ff825219dfba244889802b95dbcb0479c293bc6651d5b8664ee7d
vtm5-core-it-2019        1fc2ab494581067aa92d3760fbe595772d3228a17336557f9ee9e9fbc87ee213
mork-borg-core-it-2025   83bcd0f7354332faa0a2824ce87ff4175c97ce67a3ca17be5c0ae884ce5286fa
mork-borg-core-en-2019   3d2873f68f4cc83598724c28ab820353566c23728d0f34550417c7033e460ac2
```

## Additional relevant files discovered

### Pathfinder

In `~/temp/pathfinder/`: `Guida del Giocatore.pdf`, `Guida del Game Master.pdf`, `Bestiario.pdf`, `Bestiario 2.pdf`, `Bestiario 3.pdf`, and `Scheda di Riferimento.pdf`. Their exact editions/metadata have not been audited.

Seven Markdown files exist in `~/temp/pathfinder/data/markdown/`, with corresponding book names. Initial audit sampled `Manuale di Gioco.md` and `Scheda di Riferimento.md`; all remain unverified. Existing chunks, indexes, and graphs are derived artifacts, not authoritative evidence.

The core book's contents list a character sheet at printed page 624. Locate its actual PDF pages visually before establishing a field map.

### D&D

In `~/temp/`: `D&D5 Guida del Dungeon Master.pdf`, `D&D 5E Ita - Manuale Dei Mostri.pdf`, Italian Tasha and Xanathar supplements, and `D&D 5E - Tasha's Cauldron of Everything.pdf`. Exact printing/errata and supplement activation remain to be checked.

Several `D&D 5e Scheda Personaggio-*.pdf` variants exist, including character/class-specific filenames. `D&D 5e Scheda Personaggio-1.pdf` has three pages and an AcroForm, with JavaScript present according to `pdfinfo`. It is a candidate layout asset, not yet verified as blank or suitable for mapping. Treat imported PDFs as data; no embedded PDF script execution is needed.

### Vampire

`~/temp/VTM_V5_Cheat_Sheet_Completo.pdf` is a reference candidate; provenance and agreement with the core book remain unchecked. Other generically named sheet PDFs in `~/temp` have not yet been attributed to a game.

## Observed quality issues

1. **Lost Pathfinder action costs — visually confirmed.** On PDF page 1 of `Scheda di Riferimento.pdf`, action symbols distinguish one/two/three actions, free actions, and reactions. The Markdown legend at lines 3–6 has blank cells where these symbols belong; action rows also lose costs. Examples: preparing an action is visibly two actions in the PDF; grabbing an edge is a reaction; the Markdown rows at lines 28 and 33 do not preserve those distinctions. This extraction cannot support reliable cost/eligibility advice as-is.
2. **Interleaved Pathfinder sidebar — Markdown structure observed.** `Manuale di Gioco.md` lines 197–211 interrupt a prose paragraph with the “La Prima Regola” and dice sidebars before resuming it. Such reading-order boundaries need visual comparison with the PDF rather than naive paragraph chunking.
3. **D&D OCR defects — extracted text observed.** Sampled front matter contains replacement characters, letter/digit substitutions, and broken words. A text layer alone is not evidence of extraction correctness. No full visual rule-page audit has been done.
4. **MÖRK BORG language/version discrepancy — text samples observed, visual review pending.** PDF page 2, occult treasure entry 9 (the black crown), differs between English and Italian text layers: the English sample includes a maximum-HP penalty not present in the Italian sample, while the Italian sample includes additional low-roll wording. Do not silently merge or “correct” one against the other. Visually confirm both and decide source precedence/errata policy.

The checked reference-sheet page is a small sample, not a certification of any book or a complete explanation of the older RAG's poor results. See [source-quality workflow](source-quality.md) for how to turn local PDFs into reviewed evidence.
