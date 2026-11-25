# Bundled browser assets

`htmx.min.js` is htmx 2.0.8, downloaded from https://unpkg.com/htmx.org@2.0.8/dist/htmx.min.js.
It is served locally and embedded in the executable; no CDN or Node.js toolchain is needed at runtime.

htmx is third-party software under the Zero-Clause BSD license, included as `htmx-LICENSE`.
`app.css` is Mimik's application styling. `nav.js` closes the header menus on an outside click or Escape.
`master-live.js` (the Master workspace's live list around the browser's native `EventSource`) and
`sheet-seen.js` (the automatic opened acknowledgement) are Mimik's own modules, written for the
strict sheet policy; the htmx SSE extension was tried first and cannot run under it (DEP-33), so
it is not bundled.

`htmx.source.js` is the preferred-form upstream source for the incorporated htmx
2.0.8 asset, retrieved from https://github.com/bigskysoftware/htmx/blob/v2.0.8/src/htmx.js.
It is included in corresponding-source archives, not separately loaded by the runtime.

`response-targets.min.js` and `response-targets.source.js` are the maintained htmx
response-targets extension 2.0.4, retrieved from its versioned npm distribution.
Its BSD-0-Clause license is preserved in `response-targets-LICENSE`. The sheet form
uses it only to display its409/422 fragments without losing drafts; no custom
client-side validation or game logic was introduced.

The root `logo.png` is owner-provided application artwork, kept intact and embedded
for the header. It is an explicit project-asset exception to the corpus-image
exclusion policy. Authorship/distribution rights are not inferred from the upload;
confirm those before a public release. No manual/sheet artwork is bundled.
