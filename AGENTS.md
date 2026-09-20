# Engineering rules

- Treat `docs/rcp-v0.md` as the canonical continuity architecture. Distinguish
  its locked decisions from its explicitly open decisions.
- Keep the RCP core independent of agent harnesses. Do not deepen the current
  kernel-type wire coupling; replace it when a second harness proves the shared
  boundary.
- Across RCP delivery boundaries, persist admitted data before acknowledging
  it and make retries idempotent with stable identities.
- Order effects so a rejection leaves no residue. Run every validation before
  the first irreversible effect; a failed operation leaves no row, file, or
  directory behind, and a retry adopts or converges instead of duplicating.
- Ship the smallest complete change that proves the required behavior.
- Do not add a contract field until the runtime consumes it or a test proves it.
- A change is transitive. A rename, removal, or shape change is complete only
  when callers, consumers, fixtures, error text, documents, and worked examples
  moved with it; grep the old name, the old number, and the old shape before
  calling it done, and treat a worked example in a document as code that must
  still load.
- Do not add an abstraction for a single implementation unless it enforces a
  concrete invariant.
- Give every artifact one owner. When a table, type, predicate, or default
  gains a new home, delete the old one in the same change; where nothing
  mechanical enforces single ownership, add a test that fails on a duplicate
  or an orphan.
- Prefer the standard library and existing dependencies over a new package.
- Keep provider, surface, and product policy outside the kernel.
- Keep production modules below 500 lines; test files are exempt. Crossing the
  line is a trigger to name the second responsibility or delete something, not
  to split at the counter; a module whose bulk is data rather than logic is
  judged by its data.
- Keep at most one canonical architecture document per module. Do not add
  step, status, handoff, or implementation-summary documents; code and tests
  are the execution truth.
- Never state a guarantee the code does not enforce. Comments, error text, and
  architecture documents describe what the code and its tests do; if a property
  matters, encode it in a test.
- Test the real execution path with deterministic boundaries.
- Prove a new test can fail. Revert the change, watch the test fail, restore
  it; a test that cannot fail hides the defect it claims to protect.
- Treat newly reachable code as new code. When a change turns a failing,
  refused, or unreachable path into a working one, re-audit everything
  downstream of it in the same change.
- Treat warnings as errors. Before handing off code, run:
  `cargo fmt --all -- --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`, and
  `cargo test --workspace`.
- Record the source commit and license before adapting upstream code.
- Do not add AI tools or models as commit co-authors or add generated-by
  footers to commits and pull requests. Preserve required upstream license
  and source attribution.
- For non-trivial repository work, apply R-Stack. These engineering rules
  remain authoritative.
- Keep exactly one consolidated backup of the immediately previous release.
  After the next release passes health checks, replace that backup and delete
  older release archives, snapshots, and unused versioned binaries.
- Deploy the current runtime only. Do not add historical-runtime fallbacks,
  dual-version compatibility, or automatic rollback unless explicitly requested.
