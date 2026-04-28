/**
 * Component entrypoint: importing each agent module is enough — the
 * top-level `defineAgent(...)` call auto-registers the agent with the
 * runtime so the `agent-guest` host exports can discover and invoke it.
 */
import "./counter-agent.js"
import "./caller-agent.js"
import "./sqlite-counter-agent.js"
import "./pg-counter-agent.js"
import "./mysql-counter-agent.js"
import "./host-features-agent.js"
import "./booking-saga-agent.js"
import "./inventory-saga-agent.js"
import "./quota-tester-agent.js"
import "./webhook-agent.js"
