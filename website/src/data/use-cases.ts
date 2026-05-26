// Use-cases page copy — single source of truth for all visitor-facing
// prose on /use-cases.
//
// Editorial discipline (v2 compression):
//   intro:         20-24 words, 1 sentence
//   examples:      4 bullets at 9-11 words each
//   fits.title:    short noun phrase (~5 words)
//   fits.body:     1 sentence, 11-19 words
//
// Each bullet earns its place by naming a concrete capability AND
// connecting it to the intro's hard part. No internal capability
// codes; no marketing slogans; no claims a competitor could make
// unchanged.

export interface FitItem {
  title: string;
  body: string;
}

export interface FeaturedUseCase {
  title: string;
  intro: string;
  examples: string[];
  fits: FitItem[];
}

export interface ClassicDE {
  title: string;
  overview: string;
  examples: string[];
  fits: string[];
}

// =============================================================================
// Page metadata + hero
// =============================================================================

export const meta = {
  title: "Use Cases — What you build with Golem",
  description:
    "From per-user AI agents to multi-step business workflows, here's what Golem is built to run — and the architectural reasons why.",
};

export const hero = {
  eyebrow: "Use cases",
  heading: "What you build with Golem",
  lede: "Golem is the durable agent runtime. From per-user AI agents to multi-step business workflows, here's what it's built to run — and the architectural reasons why.",
};

// =============================================================================
// Section headers
// =============================================================================

export const featuredSection = {
  eyebrow: "AI agents",
  heading: "Featured agent use cases",
};

export const classicSection = {
  eyebrow: "Classic durable execution",
  // Section heading comes from classicDE.title below to keep it co-located
  // with the rest of the wide-block copy.
};

// =============================================================================
// Featured agent use cases (6 cards)
// =============================================================================

export const featured: FeaturedUseCase[] = [
  {
    title: "Customer support agents",
    intro:
      "A durable agent per conversation that survives crashes, redeploys, and migrations — held open for hours or days across chat, email, voice, and ticketing.",
    examples: [
      "Support triage agent that resolves common tickets and escalates the rest",
      "E-commerce concierge handling refunds, shipping status, and order changes",
      "SRE incident copilot that triages alerts, runs playbooks, and pages humans",
      "Per-conversation chatbot for SaaS customer success and onboarding",
    ],
    fits: [
      {
        title: "One agent per conversation",
        body: "Each conversation is a stable, single-writer agent with durable state — no router, no registry, no orchestrator code.",
      },
      {
        title: "Replays byte-identically",
        body: "Every effect is journaled and replay is deterministic — state survives crashes, redeploys, and migrations.",
      },
      {
        title: "Zero-cost idle",
        body: "Idle agents suspend entirely — no memory, no compute — and wake instantly on the next event.",
      },
      {
        title: "Wait for humans, no timeouts",
        body: "Conversations pause indefinitely for human approval — hours-long escalations and multi-day reviews are first-class.",
      },
    ],
  },
  {
    title: "Coding & dev agents",
    intro:
      "Agents that read, write, test, and ship code — running for hours, executing untrusted AI output, pausing and replaying without losing plan state.",
    examples: [
      "Autonomous PR agent that reviews code and proposes fixes",
      "Prompt-to-application builder spinning up backend and frontend together",
      "Legacy-code modernization across COBOL, ABAP, and APEX targets",
      "CI/CD remediation agent that diagnoses failures and proposes patches",
    ],
    fits: [
      {
        title: "Long-running by construction",
        body: "Multi-hour, multi-day workflows are the default — no serverless timeout to design around, no manual checkpointing.",
      },
      {
        title: "Sandboxed by construction",
        body: "AI code runs inside WebAssembly — isolation at instance cost, no separate sandbox service to wire up.",
      },
      {
        title: "Pause, branch, rewind, replay",
        body: "Every effect lives in the journal — pause, branch, rewind hours of work, or replay the whole session for debugging.",
      },
      {
        title: "Built for coding agents",
        body: "Golem 1.5 ships agent-skills, AGENTS.md, and weekly benchmarks against real coding workloads.",
      },
    ],
  },
  {
    title: "Internal data copilots",
    intro:
      "Agents inside an enterprise system of record, where multi-hour reasoning, durable RAG ingestion, and per-user permissions break naive implementations.",
    examples: [
      "HR / recruiting copilot searching profiles and drafting outreach",
      "FP&A planning copilot with NL-to-SQL across financial systems",
      "Internal docs and knowledge agent grounded in wiki and tickets",
      "Investment-research agent over filings, reports, and analyst notes",
    ],
    fits: [
      {
        title: "Durable RAG ingestion",
        body: "Fan out millions of document parses, resume from step N after a crash, with exactly-once embedding writes.",
      },
      {
        title: "Per-user agents + permissions",
        body: "Each user gets their own agent, with permissions in the identity — cross-user leaks are structurally impossible.",
      },
      {
        title: "Multi-hour, zero idle cost",
        body: "Analyst loops run for hours over remote APIs; agents suspend on each call and consume no compute when idle.",
      },
      {
        title: "Audit trail by construction",
        body: "Every query, retrieval, and tool call appears in the same journal that drives replay — audit isn't bolted on.",
      },
    ],
  },
  {
    title: "Per-user agent fleets",
    intro:
      "One durable instance per user, tenant, or device — millions of stateful agents that must be cheap, isolated, and individually addressable at scale.",
    examples: [
      "Per-cardholder recommendation agents across millions of customers in finance",
      "Per-tenant AI agents inside a multi-tenant SaaS product",
      "Per-device operators for IoT fleets — homes, vehicles, sensors",
      "Per-property concierge in real-estate or hospitality at scale",
    ],
    fits: [
      {
        title: "Per-key agent identity",
        body: "Single-writer state per key is the runtime's shape — one stable agent per user, addressable from anywhere.",
      },
      {
        title: "WebAssembly at instance cost",
        body: "Agents use megabytes, not gigabytes — millions on commodity hardware become viable economics.",
      },
      {
        title: "Isolation between every agent",
        body: "WASM sandboxing between agents at MB cost — cross-tenant leaks are structurally impossible, not policy-disallowed.",
      },
      {
        title: "Migrates without losing state",
        body: "Agents move between machines as the cluster scales — same memory, same history, same in-flight tool calls.",
      },
    ],
  },
  {
    title: "Regulated & on-prem agents",
    intro:
      "Agents in healthcare, finance, and government — running for days in your VPC or on-prem, with audit replay, tenant isolation, and language flexibility.",
    examples: [
      "Healthcare claim review and medical-necessity decisioning agents",
      "Mortgage underwriting workflow with policy checks and human adjudication",
      "Financial-crime investigation across multi-agent A2A workflows",
      "FedRAMP-, HIPAA-, GDPR-compliant document processing on customer infra",
    ],
    fits: [
      {
        title: "Same software, your cloud",
        body: "Golem Cloud On-Prem ships our managed-Cloud operations stack inside your AWS, GCP, Azure, or Kubernetes.",
      },
      {
        title: "The journal is the audit log",
        body: "Every effect — config reads, tool calls, decisions — appears in the journal for byte-identical audit replay.",
      },
      {
        title: "WebAssembly isolation",
        body: "WebAssembly isolation between agents — stronger than containers, no escape vectors, no runtime tradeoff.",
      },
      {
        title: "Multi-language by default",
        body: 'Rust, TypeScript, Scala, MoonBit, Python (planned) — no "rewrite in our framework" tax for enterprise stacks.',
      },
    ],
  },
  {
    title: "Voice & chat agents",
    intro:
      "Agents inside real-time channels — voice, chat, and in-product copilots — streaming turn-by-turn while conversations persist across days and redeploys.",
    examples: [
      "AI receptionist taking inbound calls 24/7 for service businesses",
      "Slack and Discord bots with multi-turn memory across days",
      "In-product voice copilots streaming over WebRTC with low latency",
      "WhatsApp-based booking and customer-engagement agents for SMBs",
    ],
    fits: [
      {
        title: "One agent per channel",
        body: "Each call or thread is a single, durable, addressable agent — no orchestrator routing messages.",
      },
      {
        title: "Wakes byte-identically",
        body: "Agents suspend between turns at zero compute, zero memory, and wake byte-identically on the next event.",
      },
      {
        title: "History in the journal",
        body: "Every turn appears in the agent's durable state automatically — no separate conversation store, no syncing logic.",
      },
      {
        title: "Calls survive deploys",
        body: "Active calls survive new deploys and host drains — the agent migrates with all its state and in-flight tool calls.",
      },
    ],
  },
];

// =============================================================================
// Wide block: classic durable execution
// =============================================================================

export const classicDE: ClassicDE = {
  title: "Beyond agents: classic durable execution",
  overview:
    "Long-running, multi-step workflows that must not lose or duplicate effects. This is the durable-execution heartland — payments, billing, reconciliation, migrations, cron, and webhook delivery — built on sagas, compensations, exactly-once delivery, and durable timers. It predates the agent era, and on Golem, the same runtime that runs your agents runs it just as well.",
  examples: [
    "Payment sagas with compensating transactions across PSPs and ledgers",
    "Subscription billing engines with retries, dunning, and proration",
    "Cron-driven pipelines replacing Celery, BullMQ, Sidekiq, Step Functions",
    "Webhook reconciliation across systems with different ordering semantics",
  ],
  fits: [
    "Durability is the primitive — no opt-in, no manual checkpoints",
    "Exactly-once effects via the journal — retries don't double-charge",
    "Long-running natively — multi-day workflows, no serverless timeout",
    "Same runtime as your agents — one cluster, one operations surface",
  ],
};
