# Compaction

`compaction` provides a byte-pressure context transform. It summarizes only an
older settled groups of a Run through the existing budgeted model gateway and
replaces those groups in the model-visible projection. The authoritative
transcript, checkpoints, and UI history remain unchanged.

The default policy starts at 80% of the conservatively sized complete request
byte budget and targets 60%. When the effective model declares a context window,
the same thresholds also apply to a conservative token estimate after reserving
the configured output budget; the stricter byte/token target wins. System messages, the latest user request, media,
and unsupported resource references remain verbatim. Old tool rounds after
that user request can be compacted. Two recent groups are preferred; pressure
may reduce the tail to one whole group. If this mandatory tail exceeds the
60% target, a successful projection must still fall below 80% and be smaller
than the previous projection. An oversized or non-shrinking summary is rejected.
Assistant tool calls and their results are indivisible, including out-of-order
but correctly paired results. Text and JSON tool observations can be summarized;
Spill references are preserved exactly outside the generated summary.

The component is optional. `CompactionPlugin` is a thin Runtime entry point;
direct embedders may install `Compactor` as a normal `ContextTransform`.
Auxiliary summary calls share the Run's model selection, cancellation, wall
time, model-call count, token accounting, and request byte limit.

Auxiliary requests pack whole groups and measure their actual serialized size,
including JSON escaping and model options. A known model window also bounds
each auxiliary request using its own reserved output tokens; a group that
cannot fit is rejected before calling the model. Cached summaries are applied before
checking pressure, and ordinary projection summarizes only newly selected groups. Runtime
awaits `ContextTransform::finish` on completion/cancellation to remove that Run's
cache; direct users must call it themselves. Host shutdown clears any remainder.

Token pressure is approximate and never replaces the request byte hard limit.
A provider-confirmed context overflow gets one forced compaction opportunity
through `ContextTransform::recover_context`; Runtime retries only when the
resulting projection is strictly smaller. When protected recent input cannot
fit the known model window after reserving output tokens, compaction fails
explicitly instead of sending a request above the declared capacity.

If all eligible history is already summarized, forced recovery may re-summarize
that cached summary once with a smaller escaped-JSON budget. This still uses the
same budgeted gateway. An unchanged or oversized summary fails without resending
the main request; another context overflow does not start another recovery cycle.
Auxiliary usage remains in task accounting and audit, but is not reused as the
main conversation's input-size estimate. The authoritative transcript is never
rewritten by this recovery.

API 7 pinned context sources and the conservative tool envelope are included before
pressure is checked; sources never become the latest user-turn anchor. A matching
primary-request usage anchor may price the unchanged prefix, with heuristic pricing
for new tail content. Unknown/mismatched anchors use byte estimation; auxiliary
summary requests always measure their own request and output reserve.

`Compactor::state(run_id)` exports a detached `CompactionState` containing summary,
whole source-group ranges and SHA-256 fingerprints. Capture it at an awaited host
commit before `finish` removes the Run cache. `CompactionState::project` validates
and applies it to the corresponding system-free working history, including new
settled tail messages; altered or invalid ranges fail explicitly. It does not write
files or replace the host's full conversation archive. The formal Server atomically
stores and restores this state across Runs/restarts; an embedder may omit persistence.
See [context management](../../docs/CONTEXT-MANAGEMENT.zh-CN.md).

Regression coverage includes `tests/recovery.rs`; run `cargo test -p compaction`.
