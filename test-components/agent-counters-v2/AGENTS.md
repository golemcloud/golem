# agent-counters-v2

The update target for the `hot_update` tests that need a build change. Identical
to `agent-counters` apart from `SnapshotCounter::component_version`, which
returns 2 here and 1 there. The component name is the same in both manifests, so
an update can swap one build for the other underneath running agents; only the
emitted file name differs, following the convention `agent-updates-v1`/`v2`
already use.

Keep the two identical apart from that number. Any other difference makes a
state mismatch after an update ambiguous between "the update lost it" and "the
two builds disagree".
