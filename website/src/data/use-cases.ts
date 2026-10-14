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
    "From business coding agents to phone support to per-user fleets, here's what Golem is built to run — and the architectural reasons why.",
};

export const hero = {
  eyebrow: "Use cases",
  heading: "What you build with Golem",
  lede: "Golem is the durable runtime for agents, their tools, and their artifacts. From business coding agents to phone support to per-user fleets, here's what it's built to run — and why.",
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
    title: "Business coding agents",
    intro:
      "Agents that write and run small programs for people who never see the code — support, operations, analysis — running entirely inside Golem.",
    examples: [
      "Support bot that writes a script to reconcile a customer's account",
      "Analyst assistant that assembles a report from three systems",
      "Ops agent that cleans and transforms the same spreadsheet every week",
      "Prompt-to-app builder that ships a working front-end and backend",
    ],
    fits: [
      {
        title: "Runs entirely in Golem",
        body: "Shell, files, git, Node, npm, and TypeScript run inside the sandbox as isolated tool calls — no VM to sync.",
      },
      {
        title: "Guardrails it can't bypass",
        body: "A path policy and tool middleware, enforced by the host on every call, whoever wrote the code.",
      },
      {
        title: "Authority it can't widen",
        body: "A permission card names the only hosts it may reach; derivation narrows, revocation cascades.",
      },
      {
        title: "Ships its own UI",
        body: "Its artifact — a report, a dashboard, an app — is served live from the agent's own files.",
      },
    ],
  },
  {
    title: "Phone & voice agents",
    intro:
      "Agents that answer the phone and stay on the line — voice to text to LLM to voice — with every call a durable agent that keeps its place.",
    examples: [
      "AI receptionist answering inbound calls 24/7 for service businesses",
      "Phone support agent that looks up orders and hands off to humans",
      "In-product voice copilots streaming audio turn by turn",
      "Slack and Discord bots with multi-turn memory across days",
    ],
    fits: [
      {
        title: "One agent per call",
        body: "Each call is a single, durable, addressable agent keyed by call ID — no session store, no router.",
      },
      {
        title: "Audio over Durable Streams",
        body: "Idempotent appends in, resumable reads out — a dropped connection loses nothing.",
      },
      {
        title: "Speech services, durably",
        body: "Speech-to-text, LLM, and text-to-speech calls are journaled and never repeat on recovery.",
      },
      {
        title: "Calls survive deploys",
        body: "Transcripts, context, and completed tool calls survive crashes, deploys, and host drains.",
      },
    ],
  },
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
        body: "Every conversation ID has exactly one live agent instance — no router, no registry, no orchestrator code.",
      },
      {
        title: "Tools behind middleware",
        body: "Refunds and cancellations run through host-enforced approval gates the agent can't skip.",
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
        title: "Per-user agents + permission cards",
        body: "Each user gets their own agent and a permission card scoped to their data — no cross-user leaks.",
      },
      {
        title: "Read-only lookups, cached",
        body: "Read-only methods can't write or call out, and their results are cached at the executor and the HTTP edge.",
      },
      {
        title: "Audit trail, automatic",
        body: "Every query, retrieval, tool call, and authorization decision is logged automatically — that same history drives replay.",
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
        body: "Every agent ID has exactly one live instance — no fan-out, no races, addressable from anywhere.",
      },
      {
        title: "WebAssembly at instance cost",
        body: "Agents use megabytes, not gigabytes — millions on commodity hardware become viable economics.",
      },
      {
        title: "Each with its own artifact",
        body: "A per-user dashboard or portal, served live from the agent's own files, behind PKCE login.",
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
      "Financial-crime investigation across cooperating agents with a full audit trail",
      "Document processing inside your VPC for HIPAA- and GDPR-regulated workloads",
    ],
    fits: [
      {
        title: "Your infrastructure",
        body: "Run the full open-source platform in your own AWS, GCP, Azure, or Kubernetes environment.",
      },
      {
        title: "Audit log, automatic",
        body: "Every effect — config reads, tool calls, decisions — is recorded automatically and replayable, identical to the original.",
      },
      {
        title: "WebAssembly isolation",
        body: "Each agent has its own memory and filesystem, no system calls, and only the authority its cards grant.",
      },
      {
        title: "Multi-language by default",
        body: 'TypeScript, Effect, Rust, Go, Scala, MoonBit — no "rewrite in our framework" tax for enterprise stacks.',
      },
    ],
  },
];

// =============================================================================
// Wide block: classic durable execution
// =============================================================================

export const classicDE: ClassicDE = {
  title: "Not just agents — any workflow that has to survive",
  overview:
    "The workflows that have to finish — even through restarts, retries, and outages. Multi-step workflows that span days, payment flows that have to either fully complete or fully roll back, scheduled jobs that have to run on time even through crashes. It predates the agent era, and on Golem, the same runtime that runs your agents runs it just as well.",
  examples: [
    "Charge a card, update the ledger, send the receipt — all-or-nothing, even through crashes",
    "Subscription billing engines with retries, dunning, and proration",
    "Cron-driven pipelines replacing Celery, BullMQ, Sidekiq, Step Functions",
    "Webhook reconciliation across systems with different ordering semantics",
  ],
  fits: [
    "Durability is the primitive — no opt-in, no manual checkpoints",
    "No effect ever fires twice — retries don't double-charge",
    "Long-running natively — multi-day workflows, no serverless timeout",
    "Same runtime as your agents — one cluster, one operations surface",
  ],
};
