---
name: renoa-code-review
description: Investigate defects introduced by a pinned pull request, challenge candidate findings and remedies, and produce evidence-backed review findings using the Host's report format.
---

Review behavior across the change, callers and external boundaries. Use the
tests to understand intended behavior, then check whether they exercise the
real failure path. An assertion that mirrors a new implementation is weaker
evidence than a boundary test that distinguishes the old and new behavior.

For each plausible defect, establish the trigger, the affected execution path,
the consequence and why the pinned change introduces it. Follow callers far
enough to find recovery, validation or ownership checks that could disprove the
claim. Compare the merge-base implementation when the patch alone is ambiguous.
Inspect relevant configuration and CI files too; use `include_hidden: true`
with grep/find for paths such as `.github/`.

Challenge the remedy as carefully as the finding. A proposed fix must preserve
the surrounding contract: stable identities, original deadlines, durable
admission, cancellation, ownership and resource cleanup where applicable.
Check failure before and after persistence or external effects. If the correct
implementation is uncertain, describe the behavior that needs to change rather
than prescribing an unverified patch.

Distinguish operational inconvenience with a documented recovery path from
data loss, security exposure or a routinely broken primary workflow. Assign
priority from the demonstrated impact and likelihood. Do not elevate every
error path to P1. Exclude style preferences, hypothetical issues with no
reachable trigger, and unrelated pre-existing defects.

Keep a concise factual working record in ordinary assistant messages during
investigation: examined paths, supported candidates, rejected hypotheses and
their counterexamples, and unresolved questions. Keep this separate from
private reasoning so compaction can preserve it. Resume with targeted source
reads after compaction instead of treating a summary as exact evidence.

For final validation, try to falsify each candidate using pinned source and
relevant callers. Remove duplicates and unsupported claims, correct unsafe
remedies, and report the actual verification gaps. Do not claim comprehensive
coverage just because the available checks pass. Use the Host's final report
schema; this skill neither grants tools nor authorizes code changes.
