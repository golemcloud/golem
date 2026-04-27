/**
 * Component entrypoint: importing each agent module is enough — the
 * top-level `defineAgent(...)` call auto-registers the agent with the
 * runtime so the `agent-guest` host exports can discover and invoke it.
 */
import "./counter-agent.js"
import "./caller-agent.js"
