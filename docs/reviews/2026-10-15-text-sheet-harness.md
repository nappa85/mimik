# Text-only model–sheet harness correction

User explicitly rejected vision-model wiring and clarified Mimik as a harness for
the model to dialogue with the sheet. JavaScript may support the web interaction;
no additional scripting language is wanted.

Independent read-only reviewer `ses_ec1f90239ffeqKcx3OgFnxZf8R` approved the revised
plan, its bounded conceptual tool extension and the resulting product-doc changes.
Scope: experience-validation-plan, roadmap, model-assisted-games, discovery and
TODO; no runtime/provider calls, tests or private asset/database/credential access.

Findings/disposition:

- Keep HTML/CSS with stable bindings a proposal; confirmed direction is web-native
  template/text context, not an already selected file contract. Incorporated.
- Join parsed labels/groups/bindings to canonical current values/types/entry IDs;
  DOM serialization alone can contain stale defaults. Incorporated with version pins.
- Uploaded markup is data by default; parser alone does not establish safe rendering
  or authorize active features. Trusted application JavaScript is distinct from
  uploaded scripts; maintained controls selected in technical plan. Incorporated.
- Remove mandatory vision/image discovery and expression-DSL prerequisites in current
  roadmap. Historical human source-image comparison remains accurately distinct.
- Conceptual read_template/read_sheet/lookup_rules/ask_user/propose_sheet_patch
  operations need caller-based authorization/private filtering, bounded schemas and
  revision checks; proposals require explicit user application, never autonomous
  writes/legality or arbitrary JS/SQL/file access. Incorporated.

Final doc-change verdict GO, no blocking findings. Two nonblocking older references
to generic Rust rendering and expression/image wiring were aligned with the approved
JS/Rust/text-context boundary. Implementation remains pending confirmed template
contract, model execution mode/assets/save policy/scorecard and independently
reviewed technical plan. No shipping code/binary changed; no live inference claimed.
