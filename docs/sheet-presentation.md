# Adaptive HTML character sheets

## Direction

The user selected recreating the original PDF sheet layout in **HTML/CSS**, avoiding fixed PDF field sizes as the main editing surface. Preserve recognizable section order, grouping, labels, and visual hierarchy while allowing content to grow. This is structural/visual fidelity with adaptive sizing, not a promise of pixel-identical fixed pages for every screen/content length.

## Implementation approach

- Map official fields to canonical versioned character data, not to PDF coordinates.
- Use semantic headings, fieldsets, labels, and repeatable lists; CSS Grid/Flexbox reproduce desktop grouping and reflow on narrow screens.
- Long notes, descriptions, equipment, powers, and abilities expand vertically. Repeated entries add rows/cards; no fixed maximum PDF box or clipping.
- Use native growing-textarea support where available (`field-sizing: content`); retain manual resizing/minimum heights as a fallback. The current prototype now uses that CSS progressive enhancement.
- Keep rule explanations adjacent to relevant fields or in accessible expandable details so reference content does not crowd the sheet.
- htmx handles server-rendered edits on the native editor; no frontend framework is needed for the layout.
- Runtime definition sheets (private campaign sheets) save with a full-page POST/redirect/GET. Their derived values and advisory checks are computed from the saved snapshot and update on save and reload; there is no live recompute while typing. Derived values are read-only text with a "Computed" label, never form controls, and their expressions and rule IDs sit in collapsed "How this is computed" disclosures. The native htmx editor is unchanged.
- Build print styling separately: paginate long sections, repeat relevant headings, and avoid hiding overflow. Browser print/PDF output can be considered later; filling/exporting the publisher's original PDF is not required for the interactive editor.

## Source assets and verification

The owner supplied `/home/marco/temp/MB_Character sheet.pdf`, a one-page landscape blank MÖRK BORG sheet. Its recognizable field groups informed the adaptive editor; the sheet's precise publication/version and full fidelity remain unverified. Logos, artwork, fonts, and decorative assets retain their own rights; recreate structure with host-controlled HTML/CSS and local assets as appropriate.

Verify representative long descriptions, many inventory/power entries, narrow screens, keyboard navigation, labels, and print overflow. Screenshot/layout checks should compare recognizable sections, not enforce fixed dimensions that defeat the content-expansion requirement.

## Current status

Mimik's structured editor has adaptive three-group organization, long-text fields, responsive columns and native auto-growth. Automated Chromium checks exercise actual htmx editing, long-text persistence, desktop/mobile reflow and preservation of invalid/stale drafts. No approved official-layout, print, or cross-browser completion is claimed.
