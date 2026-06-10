---
title: "The Rise of the Agent Runtime"
date: "2026-06-05"
author: "John A. De Goes"
tags: ["Industry Articles", "AI Agent"]
slug: "the-rise-of-the-agent-runtime"
draft: false
---

## Billions of Programs, Invisibly Written

The salesperson at BBVA starts her day. A customer is on the calendar for eleven. She types one line into ChatGPT Enterprise: *pull a one-page summary of this client's last 90 days, flag anything unusual.* Sixty seconds later, the summary is on her screen. She skims it, edits two lines, and walks to the meeting.

She does not know that a TypeScript program was written for her, that it pulled in three npm packages it had never seen before, that it ran, was edited once, and ran again. She knows only that the summary arrived.

According to OpenAI's [*State of Enterprise AI 2025*](https://openai.com/index/the-state-of-enterprise-ai-2025-report/), published December 8, 2025, BBVA *"regularly uses more than 4,000 GPTs."* Across the report's enterprise sample, weekly users of Custom GPTs and Projects have grown roughly 19× year-to-date. About one in five enterprise ChatGPT messages now flows through a Custom GPT or Project. Coding-related messages from non-engineering functions are up 36%.

In conventional framing this looks like another generation of productivity software — useful, broad, unremarkable.

What BBVA has, without intending it, is countless small programs being authored — and re-authored, every week — on behalf of people who do not think of themselves as programmers. A salesperson asks for a customer summary, an analyst asks for a pricing diff, a marketing lead asks for a brochure assembled from a folder of headshots — and in each case a small program is authored to produce the result. What the user receives is the summary, the diff, or the brochure — not the program that produced it.

Every one of those programs runs on some infrastructure. Inside a sandbox, against some pile of data, under some authority, journaling an audit trail — or not. Multiply that picture by every Fortune 500 in the world.

The dominant use of AI in 2026 is a coding agent, even though most users do not perceive it as one — and that shift is happening in the substrate beneath them rather than in the message traffic above.

## Where the Substrate Is Breaking

The substrate has serious problems, spanning security, efficient isolation, reliability, and governance.

### Security: The Trifecta Outruns Detective Controls

The [MIT NANDA initiative's 2025 study](https://fortune.com/2025/08/18/mit-report-95-percent-generative-ai-pilots-at-companies-failing-cfo/) of generative-AI pilots reported that 95% delivered no measurable return, with brittle integrations and shadow AI both named among the dominant failure modes. The [OWASP LLM Top 10](https://owasp.org/www-project-top-10-for-large-language-model-applications/) (2025 edition) catalogs a steady accretion of failure modes: prompt injection, sensitive-information disclosure, supply-chain compromise, excessive agency.

Simon Willison's three-noun summary of the structural problem — the [*lethal trifecta*](https://simonwillison.net/2025/Jun/16/general-analysis/) of an agent that reads attacker-controlled input, holds privileged tools, and can phone home — is widely used in the industry. New universal jailbreaks ship at the model layer every quarter, and detective controls cannot keep up with that cadence.

### Isolation: Too Small, or Too Heavy

Cloudflare's Workers platform caps each isolate at [128 MB of memory](https://developers.cloudflare.com/workers/platform/limits/) — efficient for stateless request-response, insufficient for an agent that must hold its own working memory, install dependencies, and run them — and in June 2025 the company [added container-class Sandboxes alongside the V8 isolates](https://blog.cloudflare.com/sandboxes-ga/), reaching general availability in April 2026. Even Cloudflare concluded that lightweight sandboxing alone was not enough.

The [post that open-sourced the Claude Code sandbox](https://www.anthropic.com/engineering/claude-code-sandboxing) approached the same gap from the other side: the new runtime is designed to give agents an isolation surface without paying a container's startup and management cost. Cloudflare moved toward heavier isolation by adding containers; Anthropic moved toward lighter isolation by stripping the sandbox down. Both were converging on the same missing runtime tier from opposite directions.

### Reliability: Side Effects Get Re-Issued

Agents now execute side-effecting tools — refunds, emails, payments, travel bookings — and the runtime beneath them rarely guarantees those calls are exactly-once. A downed or interrupted agent that is resumed will re-issue the refund.

Inngest's February 2026 essay [*Durable Execution: The Key to Harnessing AI Agents in Production*](https://www.inngest.com/blog/durable-execution-key-to-harnessing-ai-agents) — whose central slogan is *"Durable Execution is the AI Agent Harness"* — names the problem directly: AI agents combine every failure mode durable execution was designed to fix — long-running processes, expensive non-idempotent side effects, probabilistic behavior, flaky tool calls.

The entire durable-execution category — Temporal, Restate, Inngest, Trigger.dev, and DBOS — has repositioned around AI agents in 2025–2026 for exactly this reason.

### Governance: The Audit Log Is Now Mandatory

Governance is the one regulators are now codifying.

[EU AI Act Article 12](https://artificialintelligenceact.eu/article/12/) mandates *"automatic recording of events (logs)"* for the entire lifetime of high-risk AI systems. The [NIST AI Risk Management Framework](https://www.nist.gov/itl/ai-risk-management-framework) treats traceability as a first-class control. The [OpenTelemetry GenAI semantic conventions](https://opentelemetry.io/docs/specs/semconv/gen-ai/) began stabilizing in late 2025. Built-in auditability is becoming a regulatory requirement.

Taken together, these pressures point at the same gap: there is no runtime tier built for what agents are supposed to do.

## From Per-Seat to Per-Outcome

The substrate shift is also visible in pricing, as the Software-as-a-Service model slowly gives way to its successor.

Sierra, the customer-support agent company co-founded by Bret Taylor, was valued at $15.8B in May 2026 against roughly $200M of annualized revenue, and by December 2025 [Sierra claimed its AI was reaching more than 95% of US Black Friday shoppers](https://www.benzinga.com/news/topics/25/12/49189590/) through its retail customers. It does not charge for software. It charges per resolved support issue. Intercom Fin charges roughly $0.99 per resolution and ships with a million-dollar performance guarantee; Zendesk meters its automated resolutions at $1.50; ServiceNow [launched an Autonomous Workforce in February 2026](https://newsroom.servicenow.com/press-releases/details/2026/ServiceNow-launches-Autonomous-Workforce-that-thinks-and-acts-adds-Moveworks-to-the-ServiceNow-AI-Platform/default.aspx) that assigns agents to named roles — L1 Service Desk AI Specialist, Employee Service Agent, Security Operations Analyst — and reports that more than 90% of its own internal IT requests now flow through them.

The largest enterprise-SaaS company in the world sells Agentforce under three concurrent pricing models, [published on its pricing page](https://www.salesforce.com/agentforce/pricing/): $2 per conversation, roughly $0.10 per action via Flex Credits, and $125 per user (rising to $150 for the Industries tier) as "digital labor." Industry pricing trackers capture the same churn at scale: per [SaaStr's summary](https://www.saastr.com/salesforce-now-has-3-pricing-models-for-agentforce-and-maybe-right-now-thats-the-way-to-do-it/) of Growth Unhinged's *State of B2B Monetization 2025*, seat-based primary pricing dropped from 21% to 15% in twelve months and hybrid models surged from 27% to 41%; the PricingSaaS 500 Index over the same window measured 126% year-on-year growth in credit-based pricing. Forrester's read on the bifurcation is that copilots stay seat-priced because their usage is tied to humans, and workflow-automation agents migrate to outcomes because their work is not.

The chargeable unit is no longer the application or the seat but the work an agent completes — a resolved support case, a closed sale, a posted reconciliation. The agent produces the unit; the tool, the skill, and the capability are what the agent uses, is configured with, and is authorized to invoke. The agent itself stays custom to each deployment; what trades across organizational boundaries is the equipment the agent uses. Salesforce's own [AgentExchange marketplace](https://salesforcedevops.net/index.php/2025/03/04/salesforce-launches-agentexchange-marketplace/) is built around reusable Agent Templates rather than finished agents.

This is the commercial signal of the same shift the substrate analysis identified. When the chargeable unit is the agent task rather than the user-month, every problem in the prior section becomes more acute. A security failure compromises a billable transaction, not a seat. A re-issued side effect is a double charge, not a silent retry. Idle-memory overhead has no margin to absorb it when the seat itself is gone. Auditability becomes part of what is billed, not just what is reported.

## Everyone Is Building the Same Kernel — Badly

Several vendors are building pieces of this substrate without describing it as such.

In October 2025, Anthropic open-sourced the sandbox runtime behind Claude Code — the same post referenced above, which characterized effective sandboxing as requiring *"both filesystem and network isolation"* because without it *"a compromised agent could exfiltrate sensitive files like SSH keys,"* and which existed at all to give agents an isolation surface *"without the overhead of spinning up and managing a container."*

OpenAI shipped Guardrails in its [Agents Python SDK](https://openai.github.io/openai-agents-python/guardrails/); AWS shipped [Bedrock Guardrails](https://docs.aws.amazon.com/bedrock/latest/userguide/guardrails.html); Azure shipped AI Content Safety and Prompt Shields; NVIDIA's NeMo Guardrails wraps Colang rails — input, dialog, retrieval, execution, output — around the model and the tool calls as a separately addressable runtime layer.

An entire startup category — Lakera, Lasso, HiddenLayer, Robust Intelligence — has emerged to sell *AI firewalls*. [Cisco completed its acquisition of Robust Intelligence in September 2024](https://newsroom.cisco.com/c/r/newsroom/en/us/a/y2024/m08/cisco-completes-acquisition-of-robust-intelligence.html), establishing the category as tier-one enterprise security rather than a developer-tool feature.

A second cohort is building the same layer from a different starting point. Temporal, Restate, Inngest, Trigger.dev, and DBOS — the durable-execution platforms that ran a decade of background workflows — have publicly repositioned themselves as the durable layer beneath agent SDKs. [Temporal's May 7, 2026 blog post](https://temporal.io/blog/temporal-sandbox-orchestration-harness-the-missing-layer-for-running-agents) states the point directly: *"sandbox orchestration is a missing layer in agent infrastructure, and right now, everyone is rebuilding it from scratch."*

From the framework side, LangGraph — tens of millions of monthly downloads, with Klarna, Uber, and LinkedIn in production — sits inside an October 2025 LangChain reorganization that publicly names an orchestration runtime and a deployment platform underneath the framework. The same month, AutoGen and Semantic Kernel were consolidated into a [unified Microsoft Agent Framework](https://devblogs.microsoft.com/foundry/introducing-microsoft-agent-framework-the-open-source-engine-for-agentic-ai-apps/), with a Durable Task extension added in early 2026.

In June 2026, Anthropic published [*A harness for every task: dynamic workflows in Claude Code*](https://claude.com/blog/a-harness-for-every-task-dynamic-workflows-in-claude-code). Claude Code can now write its own harness on the fly as a JavaScript file, spawning sub-agents under model-routing decisions the parent made seconds earlier. The patterns Anthropic catalogs — classify-and-act, fan-out-and-synthesize, adversarial verification, tournament, quarantine — are published as a named-pattern inventory. The same post concedes durable resume as a first-class feature: *"resuming the session will allow the workflow to pick up where it left off."*

The orchestration code an agent runs is now itself written by an agent — the upper layer is no longer hand-authored framework code, but per-task code the agent emits and discards.

While the upper layer dissolves into agent output, the lower layers are hardening into shared horizontal standards. Google [donated A2A to the Linux Foundation on June 23, 2025](https://developers.googleblog.com/en/a2a-a-new-era-of-agent-interoperability/). On December 9, 2025, the Linux Foundation stood up a new [Agentic AI Foundation](https://www.linuxfoundation.org/press) to host MCP (donated by Anthropic), AGENTS.md (donated by OpenAI), and goose (donated by Block).

The parallel is clear: HTTP, Kubernetes, the 2014–2020 API economy in which Twilio, Stripe, Plaid, Algolia, and SendGrid each won by becoming horizontal infrastructure rather than vertical apps. The framework cohort is reaching down into orchestration and durability; the durable-execution cohort is reaching up into agents. They are converging on the same tier from opposite ends.

Every model vendor is privately rebuilding fragments of the same runtime, and none of them ships the whole thing. What each is trying to build is a runtime sized for the new unit of computation — call it the *agent execution kernel*, though the name matters less than the role.

A second implication of the *harness for every task* pattern is structural: if the harness can be regenerated per task, then the only safety guarantees that survive across regenerations are the ones the harness cannot edit. The harness chooses what the agent does; the kernel decides what the agent *can* do.

Every AI-firewall and LLM-gateway product on the market today runs alongside the agent — it can be bypassed by any agent that invokes the underlying API directly, and is configurable by an application developer or, increasingly, by the agent's own per-task code. Only capability denial at the host boundary cannot be circumvented by the agent's code.

What decides the category is whether enforcement is structural — built into the runtime the agent cannot edit — rather than configurable from inside the application. Web Application Firewalls grew into a multi-billion-dollar category because in-application defenses against the OWASP Top 10 were not enough; the same argument applies directly to the OWASP LLM Top 10.

## Golem: All Seven Primitives in One Runtime

The fragments those vendors are reconstructing add up to seven primitives, each answering one of the substrate failures or unmet needs the prior sections named:

1. A megabyte-class per-identity sandbox with its own filesystem solves the cost-of-isolation problem Cloudflare and Anthropic converged on.
2. Capability-based authorization solves the lethal trifecta and the OWASP excessive-agency failure mode.
3. Host-mediated tool middleware solves the bypass problem in-process middleware cannot.
4. Opaque secrets solve OWASP's sensitive-information disclosure.
5. Deterministic durable execution solves the non-idempotent side-effect problem Inngest named.
6. Per-agent storage gives agents the working memory the 128 MB-class isolate could not.
7. A runtime-produced audit journal closes the loop the EU AI Act and NIST require.

The seven only solve the substrate problem when they are present in the same runtime. We built Golem to do exactly this.

In Golem, an agent is a stateful sandbox per identity — a WASM instance measured in megabytes, suspend-to-zero (idle agents consume no memory), resumed deterministically by replaying its own oplog.

Tools wrap any CLI, any HTTP service, or any MCP server behind a typed RPC contract callable from any agent language, with CLI-shaped metadata an LLM has already seen ten thousand examples of in its training corpus.

Tool middleware is a `tool → tool` transformation the runtime enforces in front of every effect, at the host-import boundary, which the agent cannot bypass. Secrets are opaque handles whose plaintext never enters the agent's process unless a capability grants the right to reveal.

Capabilities — *cards*, in the spec — are host-minted, unforgeable authorization handles. Each card carries two bounds: what the bearer is permitted to do now, and an upper bound on what the bearer may ever do under composition with other cards.

Child cards are strictly narrower than their parent on both bounds; revocation cascades down the derivation tree. The upper bound is what makes the lethal trifecta structurally defeatable — even if a sub-agent later acquires other cards, the upper bound of its original card caps everything it can ever do, and no accumulated authority widens it. This bounds the blast radius, not the behavior: an agent can still misuse authority it legitimately holds, so irreversible effects warrant confirmation regardless. What the card removes is the trifecta's exfiltration leg — with no egress capability, attacker-controlled input has nowhere to phone home.

The oplog is the durable journal of every input, effect, and authorization decision. Per-agent storage gives every agent a private SQLite database — available today to TypeScript agents via the TypeScript SDK's `node-sqlite-extensions` on top of `wasm-rquickjs`, broadened across the other SDKs in Golem 1.6 — plus a private graph and transactional store, CozoDB, arriving shortly after 1.6, suspended with the agent and resumed with the agent.

As of mid-2026, no other widely deployed agent runtime ships per-agent embedded SQL and graph as a default primitive.

Return to the salesperson at BBVA. The one-page summary she asked for is assembled by an agent that writes a small TypeScript program to its own filesystem, installs the npm packages it needs, runs the file, edits it when the output is wrong, and runs it again.

The capability card it runs under does not grant network egress outside the bank's domain — and the card is minted by the bank's IT in the deployment manifest, not assembled by the agent. The oplog records every decision the LLM-generated code makes, and the secrets the code needs to read the customer ledger are passed to it as opaque handles, never as plaintext.

When the salesperson asks for the same summary on next quarter's accounts, the agent does not regenerate the code. It keeps the script in its own per-agent filesystem, runs it against the new inputs, and returns the new summary.

The same primitive also scales upward, to populations of agents. When the work is a population — say a thousand LinkedIn outreach campaigns — the operator's agent writes a small TypeScript program defining the task, then spawns a thousand sub-agents under capability cards that are strict narrowings of its own.

Each sub-agent's card permits sending through a single identity and nothing else: no other mailbox, no escape from its own sandbox. The runtime enforces the narrowing at spawn time, so the parent's code cannot grant the child anything the parent does not already hold. The sub-agent's oplog is the operator's audit trail for what it did.

The same capability primitive supports both the per-knowledge-worker coding agent and the thousand-instance autonomous workforce, and in both cases the runtime — not the application — produces the audit log.

Capability primitives, durable-execution primitives, and audit primitives each exist individually in today's stacks. That's not new.

What is novel is requiring all three in the same runtime. If Temporal is wired to a third-party sandbox, the workflow records what the agent did but not what it was permitted to do, because Temporal is not a runtime and cannot make such decisions. Tool middleware can be bypassed, because there is no runtime to enforce middleware policies.

Cloudflare is the closest single vendor to Golem, but runs only on its own cloud. Golem ships the same binary in managed cloud and in the customer's own data center. That same-binary-in-customer-DC property matters for regulated buyers under the EU AI Act, for data-residency requirements in healthcare and financial services, and for sovereign-cloud mandates.

What is distinctive about Golem is the intersection: capability decisions and durable effects in one journal, mediated by middleware that the agent's code cannot route around — all on a megabyte-class per-agent sandbox.

At that footprint, a commodity node can hold tens of thousands of concurrent agents rather than hundreds — the order of magnitude that makes one-agent-per-knowledge-worker-per-task feasible at all. At gigabyte-class footprints the same hardware holds hundreds, and the pattern stops being economically viable.

## Four Predictions and a Bet

If this essay's thesis is right, four shifts should be visible inside three years, and one much sooner.

**First**, outcome-priced agents will take a growing share of new enterprise AI contract dollars, up from the low single digits today. **Second**, runtime-mediated middleware — AI firewalls, capability brokers, policy proxies — will displace in-process middleware as the recommended integration point in leading agent frameworks. **Third**, per-agent durability will move from runtime-specific feature to category-wide expectation.

**Fourth**, the concrete bet: at least one frontier-model vendor other than Anthropic will ship a per-task agent-authored orchestration program inside its primary coding-agent product within eighteen months. If the pattern is structural, OpenAI, Google, xAI, or Meta ships something recognizably similar.

The world needs a durable agent runtime — an agent execution kernel — by 2029. The evidence in this essay suggests one or two platforms will end up being it. The risk of building this is bounded: the result is a capability-secure agent runtime that addresses a real problem.

If the broader reading holds, the consequence is that Golem (or something very similar to it) becomes the runtime tier on which regulated enterprises, model vendors, and knowledge-worker SaaS products run their agents.

The salesperson at BBVA will never know any of it exists — the sandbox, the capability card, the oplog. She will know only that the summary arrived.

Those odds sound good to us.
