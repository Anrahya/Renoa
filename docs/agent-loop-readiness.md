# Agent-loop readiness

This matrix prevents "agent loop ready" from hiding materially different
claims. `Ready` means the behavior is implemented and tested through the real
kernel path. `Partial` means a useful boundary exists but an important durable
behavior is absent. `Next` is the next consumer-proven slice. `Deferred` means
no contract should be added until a real consumer needs it.

| Area | Status | Proven behavior or remaining gap |
| --- | --- | --- |
| Ordered model/tool loop | Ready | Durable user, assistant, and tool messages; sequential tool calls; exact continuation after restart |
| Replaceable model and tools | Ready | Frozen named bindings and revisions; provider and workspace policy stay outside the kernel |
| Model response integrity | Ready | Only complete responses enter semantic history; tool-call identities and stop reasons are validated; proven pre-inference context and authentication rejections remain out of model-visible history |
| Tool failures | Ready | Definite failures carry stable categories and partial-change metadata back to the model; uncertain outcomes remain kernel-blocking and are not rewritten as ordinary errors |
| Effect recovery | Ready | Intent precedes dispatch; safe effects replay with the same identity; unsafe uncertain effects block honestly |
| Unknown-effect closure | Ready | Explicit host action balances tool history without changing an unknown effect into a false result |
| Durable user cancellation | Ready | Stable request identity, persist-before-signal, exact active-operation targeting, no new effects after cancellation wins, awaited cleanup, and loop-owned transcript closure |
| Local coding proof | Ready | A deterministic model drives the real guarded file and Bash tools through the kernel path |
| Local Host composition | Ready | Alpha's versioned prompt, bounded workspace rules, selected Pi model/reasoning, durable compaction, and all six local tools resolve into one frozen runtime; headless and ACP callers share one exact-turn Host path, and the headless path proves same-session selection changes |
| Transient model/tool progress | Ready | Model deltas and tool start/progress/end events flow from kernel-invoked effect adapters to a Host observer without entering the kernel contract; ACP proves live delivery before durable completion |
| Local tool lifecycle | Ready | Every built-in tool has a total deadline; cancellation waits for cleanup; Bash, ripgrep, and model bridges stop process groups; write/edit use synced atomic replacement and stale-edit detection |
| Diagnostic trace | Ready | Separate per-session SQLite records the real Host path with timestamps, durations, model TTFT, every chunk, exact translated provider payload, redacted response metadata, normalized usage/cache tokens, and typed tool flow; it is not recovery truth |
| Surface resume and close | Ready | ACP replays gapless settled kernel history with stable message IDs; close cancels and waits for quiescence; the desktop stores no duplicate transcript |
| Context projection and compaction | Ready | External strategies can construct typed plans; external projectors are sized before dispatch selection; safe-cut planning, exact persisted summaries, bounded validation, non-destructive activation, overflow fallback, cancellation, uncertainty, and restart windows are proven through the real kernel path |
| Steering and follow-ups | Deferred | Ordering against active work and admitted commands needs a real surface consumer |
| Approvals and permissions | Deferred | These remain host/product policy; a durable decision contract needs a concrete host flow |
| Parallel tool batches | Deferred | The SDK can describe parallel-safe tools, but the durable loop intentionally executes source order sequentially |
| Partial-stream recovery | Deferred | No durable prefix or partial tool-call contract exists; incomplete model streams remain uncertain |
| Cross-session context and branches | Deferred | Session-tree direction is recorded in the kernel architecture, but provenance and snapshot contracts need the first branch consumer |
| Remote continuity (RCP) | Separate | RCP remains a harness-independent connection system and is not part of loop readiness |

The next slice should still be chosen by a real product consumer, not another
speculative kernel seam. ACP now proves the complete local coding path. The UI
can consume that wire contract while core work waits for a concrete missing
behavior revealed by the surface. Any new contract should be added only when
that flow tests it end to end.
