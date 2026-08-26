# Engineering rules

- Treat `docs/rcp-v0.md` as the canonical continuity architecture. Distinguish
  its locked decisions from its explicitly open decisions.
- Keep the RCP core independent of agent harnesses. Do not deepen the current
  kernel-type wire coupling; replace it when a second harness proves the shared
  boundary.
- Across RCP delivery boundaries, persist admitted data before acknowledging
  it and make retries idempotent with stable identities.
- Ship the smallest complete change that proves the required behavior.
- Do not add a contract field until the runtime consumes it or a test proves it.
- Do not add an abstraction for a single implementation unless it enforces a
  concrete invariant.
- Prefer the standard library and existing dependencies over a new package.
- Keep provider, surface, and product policy outside the kernel.
- Keep Renoa-authored modules below 500 lines; split by responsibility, not
  arbitrary size. Do not split unmodified files in the recorded T3 Code fork
  solely to satisfy this limit. Prefer new integration modules over invasive
  edits to oversized upstream files.
- Maintain T3 Code as an upstream-tracking fork. Keep routine Renoa integration
  changes concentrated in added modules and narrow registration points so
  upstream merges stay reviewable. Deliberate product divergence is allowed;
  avoid incidental renames, moves, or mass formatting.
- Test the real execution path with deterministic boundaries.
- Treat warnings as errors. Before handing off code, run:
  `cargo fmt --all -- --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`, and
  `cargo test --workspace`.
- Record the source commit and license before adapting upstream code.
