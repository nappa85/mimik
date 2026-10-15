# Independent autonomous-development plan review

Reviewer: separate read-only general subagent session `ses_ed5e0c538ffeMGOumHX26Rai8c`.
Reviewed plan: pedantic/follow-up regressions; container-only installs; bounded Marker prototype; subsequent Rust source import/review/lookup vertical slice.

Verdict: quality step approved, extraction approved as bounded optional experiment; import/review requires a precise contract before implementation.

Required design dispositions for the source workflow:

- Separate PDF identity from extraction-run identity; require declared system/edition, original PDF indexes, extractor config/version and required hashed artifacts. Newly imported data is always unreviewed.
- Directory-only import with allowlisted names, symlink/special-file rejection and size/count limits. Store validated bytes privately, resolving browser requests by database identifiers, not paths.
- Original source page renders must be present and hash-bound before accepting a review. Container tooling renders from the supplied source PDF; core Rust need not invoke a renderer. Metadata binds declared provenance, not cryptographic proof of production.
- Preserve complete raw extraction. Review/index nonoverlapping units; never leak unreviewed children through approved parent nodes. Append-only decisions with expected revisions; corrected content separate from raw; searchable state updates atomically.
- Track page completeness separately: approving existing blocks does not establish that nothing was omitted. Search distinguishes unsupported source/edition, incomplete coverage and no reviewed match.
- Run bounded selected-page extraction with CPU-only dependencies; no credentials/cloud endpoints in extraction containers. New tool installations stay in containers. CI tools likewise run in isolated containers while maintaining Ubuntu 22.04 runtime build baseline.

Next source implementation step will use this contract and receive its own independent implementation review. No full extraction/source-review feature is approved merely by this plan review.
