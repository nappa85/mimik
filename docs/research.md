# Existing-tool research

Research date: 2026-10-11. Findings are based on the linked public pages and repository documentation, not hands-on product testing. Marketing claims about accuracy have not been independently verified.

## Comparison

| Tool | Verified overlap | Implications for Mimik |
| --- | --- | --- |
| [Foundry VTT](https://foundryvtt.com/) | Self-hosted browser-based platform; more than 200 supported game systems; extensible system/module API | Strongest existing extensible foundation. Uses JavaScript/HTML/CSS rather than an all-Rust application. Exact sheet fidelity and automation coverage must be checked per system and edition. |
| [D&D Beyond](https://www.dndbeyond.com/) | Official D&D character builder, sheets, rules, spells, equipment, and campaign tools | Useful benchmark for D&D sheet interactions and linked rules. The reviewed page does not establish a general plugin-based multi-system rules engine. |
| [Pathbuilder 2e](https://pathbuilder2e.com/) | Pathfinder 2e character builder, planner, and sheet; web and Android products | Useful benchmark for PF2e character building; not the complete multi-system companion in this brief. |
| [Demiplane](https://demiplane.com/) | Public navigation exposes games, characters, homebrew, and campaigns | Candidate to evaluate further. Client-rendered content limited the retrieved evidence; edition coverage, automation, and AI features were not verified. |
| [RPGX Proton](https://github.com/x8xid82/rpgx-proton) | Documentation describes a Windows local-AI desktop companion, Ollama management, per-world RAG, GM notes, and integration with a Foundry module | Close match for local AI plus an existing multi-system VTT. Documentation describes Electron/Node.js and proprietary paid software; the public repository contains documentation, not a reusable Rust implementation. |
| [Grounded RPG Proxy](https://github.com/briannewtonpsyd/grounded-rpg-proxy) | MIT-licensed Python proxy for PUM Companion; ingestion of owned rulebook PDFs; per-game retrieval; OpenAI-compatible interface; Gemini default | Close match for bring-your-own-manual RAG. Its documented flow is a proxy for another companion, not an official-layout sheet and deterministic validation engine. Local hosting does not imply local inference: the default uses cloud providers. |

## Discovery method and limits

Reviewed primary product pages and followed results from [GitHub repository search: rpg companion rag](https://github.com/search?q=rpg+companion+rag&type=repositories). This is a targeted initial survey, not proof that no exact competitor exists. DiceCloud and MÖRK BORG content pages returned too little readable content to support feature conclusions.

## Conclusion

Similar products exist, including a plausible Foundry + local-AI stack. No reviewed source establishes the entire requested combination: standalone Rust/htmx, official-layout sheets across the selected editions, deterministic situational eligibility, and source-grounded local Qwen explanations.

Before committing to a new application, confirm whether Rust ownership and the integrated player-focused experience justify building it, or whether evaluating an existing stack is preferable.
