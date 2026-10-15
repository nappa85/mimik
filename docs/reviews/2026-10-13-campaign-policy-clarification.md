# Campaign/account policy clarification

Documentation-only scope: `TODO.md`, `docs/discovery.md` and
`docs/collaboration-and-history.md`. No application/schema/runtime changes.

Fresh read-only plan reviewer `ses_ecb58dc7fffeCzsC6ulVCvdokH` approved recording
the user's exact decisions: original campaign creator controls additional Master
promotion/demotion; roles may be temporary or long-lived without an inferred
automatic expiry; owning player decides character transfer; Owner may disable
accounts/reset passwords with session invalidation and no implicit sheet access.

Review identified no blocking fidelity issue in the plan. It required preserving
post-membership Master sheet/history access as unanswered, since personal
ownership does not decide access after departure/removal. Implementation remains
blocked on that answer and a separately reviewed auth/campaign plan.

Fresh independent edited-document reviewer `ses_ecb5747ebfferFNuo6CftVYLMr`
approved the updates with no blocking findings: faithful decisions, no inferred
expiry or post-departure access, correct blockers and no shipped-feature claims.
No runtime tests were run or required for this requirements-only change; prior
publication artifact gates are unaffected.

## Departure clarification

User confirmed retained player read-only sheet access after departure/removal and
no sheet edits/progression outside a campaign without Master supervision. Fresh
read-only plan reviewer `ses_ecb5685c9ffeS7LcyE4nJckYGe` approved recording this,
requiring the earlier unconditional player-edit statement to be qualified.
Former-Master sheet/history access and post-departure history implications were
not inferred at that stage. Fresh edited-document reviewer
`ses_ecb5520d9ffeUkgMgLuEsQ6ll6` approved that clarification; no runtime changes/tests.

## Retained campaign linkage clarification

User subsequently confirmed that departure/removal preserves the sheet's campaign
link and freezes it; campaign Masters retain read-only sheet/history access.
Fresh plan reviewer `ses_ecb5445bdffe48dB6HPBjLoigQ` approved recording exact
campaign/ruleset linkage, player read-only sheet access, current campaign Masters'
read-only sheet/history access, and no edits/progression by either role. It
required clearing superseded unanswered-policy wording and avoiding inferred
historical grants for demoted Masters, reassignment, detachment or reactivation.
The active access-policy blocker is resolved. Next scope is a separately reviewed
auth/campaign implementation plan and library selection; history/delete/purge
decisions remain deferred. Fresh edited-document reviewer
`ses_ecb5271d3ffe0fVNX3V7c4JG4r` approved the final policies with no blocking
findings. Its housekeeping finding about the broad pending policy checkbox was
resolved by splitting confirmed access policies from deferred lifecycle decisions.
Verification was read-only document review; no runtime tests were run or claimed.
