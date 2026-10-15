# Sign-in diagnosis and integrated errors

User reports generic401 for both correct/incorrect password and wants errors inside
the UI, not bare text. Investigate with fresh synthetic accounts across loopback,
LAN and configured HTTPS proxy. Ask only nonsecret URL/login/startup details; do not
inspect/reset user database/passwords/cookies/.env/manuals or invent an account fix.

## Implementation scope

- Reuse maintained Axum/Askama/session/account infrastructure, existing strict MIME/
  document presentation selection and centralized static error normalization. No
  new dependencies or credentials protocol. Separate wrong credentials from invalid
  form/session, throttle and busy conditions in UI feedback while preserving statuses.
- Native HTML sign-in failures rerender the login page, retaining a bounded login
  name, clearing password and showing an accessible alert. Unknown/disabled/wrong-
  password accounts use the same message; never claim a specific account exists.
- Canonicalize only the submitted login's surrounding whitespace and ASCII case at
  the browser login boundary, matching the existing lowercase login identities.
  Password bytes remain exact: no trimming/normalization/minimum/profile changes.
  Retain permanent uppercase/padded correct-password and wrong-password controls.
- If CSRF/session admission fails, offer a real fresh-form GET, not an accepted POST
  or an automatic session/account change. Existing absolute expiry/revocation and
  rotation remain authoritative. Do not create a fresh session silently on failed POST.
- Browser document errors from parsing/Origin/body/admission/denied routes use a
  shared styled error page with safe static text and fixed navigation/retry links.
  Preserve trusted private-editor and credential-retry pages. Explicit API/fetch/
  wildcard clients keep status/text behavior. No return-URL reflection.
- Recoverable sign-in401/403/429/503 pages use a narrowly internal trusted marker
  so normalization retains their HTML; failures never emit session cookies or echo
  passwords. Keep anti-framing/no-store/referrer headers and Retry-After contracts.
  Storage integrity500/timeout408 errors remain real errors, with browser UI only
  where safe. No success/cookie grant after failed persistence.
- Diagnose the reported correct-password issue without assuming its cause. Confirm
  actual known-account valid login survives an incorrect attempt in loopback/LAN/
  proxy browser flow. If synthetic reproduction is unavailable, record it honestly
  and use the user's nonsecret setup details to narrow mismatch/expired form causes.

## Review and verification

Independent plan and implementation review. Failing-first permanent tests for bare
HTML-document login401, canonicalized login, expired/revoked form recovery, alert/
username retention/password clearance, quota/busy/error distinctions, API/fetch
status preservation, strict Origin/body/header handling and no-cookie/no-secret
failures. Controlled account/session races remain distinct from actual browser tests.

Full locked Rust/helper checks, strict lint/audit/notices/default build/source allowlist;
fresh unpacked delivered process/Chromium desktop-mobile and separate-client LAN/HTTPS
proxy/QR regression gates, checksums/equality/offline empty-cache build, independent
fix GO, atomic install and TODO/review handoff. User data remains untouched.

Diagnosis while preparing recovery coverage: current login checks the loaded CSRF
value only before password work, unlike the shared durable admission used by credential
redemption. Reuse `CsrfAdmission` to validate original session/deadline/payload before
verification and again before interpreting verification success/failure and rotating
the session. This implements the existing absolute-expiry/revocation contract; it
does not relax passwords or persist a replacement on failed POST. Storage/integrity
errors must not be mislabeled as credentials. Add a controlled failing-first late-
expiry login regression, then obtain independent review of this narrow addition.
