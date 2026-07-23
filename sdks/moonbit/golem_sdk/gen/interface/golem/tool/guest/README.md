Interface exported by a component that provides tools. The component
declares which tools it exposes, supplies their metadata, and accepts
invocations against a chosen leaf command of any tool.

Tools are stateless from the host's perspective: each invocation is
independent. State accumulated by the underlying agent (file-system
writes, config-store updates, etc.) persists per the agent's normal
rules and is independent of the tool calling convention.