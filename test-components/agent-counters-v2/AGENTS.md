# agent-counters-v2

The update target for chaos scenario S5. Byte-for-byte `agent-counters` with
`Counter::component_version` returning 2 instead of 1.

S5 updates the registry component from `agent-counters` to this build while
agents are running, kills an executor two seconds in, and then proves every
agent came up on the new build with its state intact. `component_version` is how
it proves the code actually executing is the new one, rather than trusting the
revision the platform reports.

Keep this identical to `agent-counters` apart from that number. Any other
difference makes a state mismatch ambiguous between "the restart lost it" and
"the two builds disagree", which is the one thing S5 must be able to tell apart.
