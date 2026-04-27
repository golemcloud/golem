/**
 * Component entrypoint: importing the agent module is enough — the
 * top-level `defineAgent(...)` call auto-registers the agent with the
 * runtime so the `agent-guest` host exports can discover and invoke it.
 *
 * This component is deployed separately from the main `agents`
 * component because the `golem:rdbms/ignite2@1.5.0` host binding may
 * not be exposed in every Golem environment. Skip deploying this
 * component when targeting an Ignite-less host.
 */
import "./ignite-counter-agent.js"
