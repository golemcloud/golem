Interface the runtime exposes to agents and tools for discovering and
invoking ambient tools — tools registered by other components in the
same Golem environment.

Mirrors the structure of `golem:agent/host`, but keyed on tool name
(rather than agent-id), and without the agent-instance constructor
step (tools are stateless invocables).