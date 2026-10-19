// Cloud-page copy — single source of truth for /cloud.
//
// Prose fields ending in `Html` accept inline markup (<strong>,
// <em>, <a>, <code>, <br>) and are rendered via Astro's `set:html`.
// Plain string fields are rendered as text.

// =============================================================================
// Page metadata + hero
// =============================================================================

export const meta = {
  title: "Golem Cloud — Pricing & Commercial Options",
  description:
    "Three ways to run Golem: open source (you run it), Golem Cloud (we run it), or On-Prem (coming soon). Paid Golem Cloud launches in the coming weeks.",
};

export const hero = {
  eyebrow: "Pricing & Commercial Options",
  heading: "Golem Cloud",
  ledeHtml: `<strong>Open source:</strong> you run it (BUSL‑1.1, transitioning to Apache‑2.0). <strong>Cloud:</strong> we run it. <strong>On-Prem:</strong> you run it with our tools and support (coming soon).`,
  statusHtml: `Paid Golem Cloud launches in the coming weeks. The Developer Preview is free today.`,
};

// =============================================================================
// Section 2 — Decision diagram
// =============================================================================

export interface PathCard {
  eyebrow: string;
  heading: string;
  body: string;
  ctaLabel: string;
  ctaHref: string;
  pill?: string;
  emphasized?: boolean;
}

export const paths = {
  heading: "Which path is right for you?",
  cards: [
    {
      eyebrow: "Try Golem",
      heading: "Open source. Free.",
      body: "The full platform — agents, tools, artifacts, accounts, and metering — and you operate it. BUSL‑1.1 today, transitioning to Apache‑2.0.",
      ctaLabel: "GitHub →",
      ctaHref: "https://github.com/golemcloud/golem",
    },
    {
      eyebrow: "Managed for you",
      heading: "Golem Cloud",
      body: "Developer Preview today, free. Paid plans with usage-based pricing launch in the coming weeks.",
      ctaLabel: "Get started →",
      ctaHref: "https://learn.golem.cloud/quickstart",
      pill: "Paid plans: coming weeks",
      emphasized: true,
    },
    {
      eyebrow: "Inside your cloud",
      heading: "Golem Cloud On-Prem",
      body: "The software we run for Golem Cloud, packaged for your own Kubernetes cluster on any cloud or on-premises. Coming soon.",
      ctaLabel: "Talk to sales →",
      ctaHref: "mailto:sales@golem.cloud",
      pill: "Coming soon",
    },
  ] as PathCard[],
};

// =============================================================================
// Section 3 — Golem Cloud (managed)
// =============================================================================

export const cloudSection = {
  eyebrow: "Golem Cloud — Managed",
  heading: "We host Golem for you.",
};

export const today = {
  heading: "Golem Cloud today",
  leadHtml: `Log in from the CLI, deploy, and your agents, tools, and artifacts are live — HTTP APIs on <code>&lt;name&gt;.apps.golem.cloud</code>, MCP servers on <code>&lt;name&gt;.mcps.golem.cloud</code>.`,
  command: `# Deploy (browser login on first use)
golem deploy --cloud
# Usage and limits
golem account usage show --cloud
golem account limits show --cloud`,
};

export const previewBanner = {
  pill: "Free during Preview",
  bodyHtml: `<strong>Golem Cloud is currently in Developer Preview</strong> — free to use, AS-IS, with no SLAs or data-retention guarantees. Preview workload data may be wiped. The Preview is intended for evaluation, experimentation, and prototyping — not production. <a href="/legal#preview">See legal →</a>`,
};

export const meteringExplainer = {
  heading: "Metering you can see",
  leadHtml: `Golem Cloud meters four dimensions, each priced independently. <strong>You pay only for what you use</strong> — no monthly base, no minimums. Usage is visible today from the CLI and the API.`,
  dimensions: [
    {
      html: `<strong>Compute</strong> — measured in <em>Golem Compute Units</em> (GCU). Same workload, same price — regardless of which node ran it.`,
    },
    {
      html: `<strong>Memory</strong> — GB-seconds of allocated agent memory while the agent is working. Idle and suspended agents accrue nothing.`,
    },
    {
      html: `<strong>Durable storage</strong> — GB-month of files on disk for durable agents while they're active.`,
    },
    {
      html: `<strong>Ephemeral storage</strong> — GB-month of filesystem scratch space for ephemeral agents during execution.`,
    },
  ],
  footerHtml: `Per-dimension prices are set at launch.`,
};

export interface LimitRow {
  label: string;
  behaviorHtml: string;
}

export const limits = {
  heading: "Limits and fair use",
  leadHtml: `Every plan enforces limits at runtime. <strong>Plan limits protect the platform; <a href="https://learn.golem.cloud/develop/quotas">application quotas</a> protect your downstream APIs</strong> — rate, capacity, and concurrency, with throttle, reject, or terminate on overage — and those are yours to define.`,
  rows: [
    {
      label: "Memory and disk per agent",
      behaviorHtml: `Hard caps. Raise up to your plan's ceiling with <code>golem account limits set</code>.`,
    },
    {
      label: "Monthly compute",
      behaviorHtml: `When the allowance runs out, durable agents pause.`,
    },
    {
      label: "Active agents",
      behaviorHtml: `Requests beyond the limit queue.`,
    },
    {
      label: "Apps, environments, components",
      behaviorHtml: `Creation beyond the limit is rejected.`,
    },
    {
      label: "Blob storage",
      behaviorHtml: `A per-account quota on data written through the blob store.`,
    },
  ] as LimitRow[],
};

export const pricingPreludeHtml = `<strong>Pay only for what you use.</strong> No monthly base, no minimums. Allowances, limits, and per-dimension prices are set at launch.`;

export interface PricingSpec {
  label: string;
  valueHtml: string;
}

export interface PricingCta {
  label: string;
  href: string;
  variant: "primary" | "secondary";
}

export interface PricingCard {
  name: string;
  priceAmount: string;
  priceUnit?: string;
  priceTagline?: string;
  pill?: string;
  emphasized?: boolean;
  specs: PricingSpec[];
  availability: string;
  cta: PricingCta;
}

export const pricingCards: PricingCard[] = [
  {
    name: "Free",
    priceAmount: "$0",
    priceUnit: "/month",
    specs: [
      { label: "Compute", valueHtml: "Monthly allowance" },
      { label: "Memory per agent", valueHtml: "Fixed" },
      { label: "Disk per agent", valueHtml: "Fixed" },
      { label: "Active agents", valueHtml: "Capped" },
      { label: "Apps / envs / components", valueHtml: "Capped" },
      { label: "Support", valueHtml: "Community" },
    ],
    availability: "Developer Preview",
    cta: {
      label: "Get started",
      href: "https://learn.golem.cloud/quickstart",
      variant: "secondary",
    },
  },
  {
    name: "Paid",
    priceAmount: "Usage-based",
    priceTagline: "Pay only for what you use",
    pill: "Coming weeks",
    emphasized: true,
    specs: [
      { label: "Compute", valueHtml: "$/GCU" },
      { label: "Memory", valueHtml: "$/GB-second" },
      { label: "Durable storage", valueHtml: "$/GB-month" },
      { label: "Ephemeral storage", valueHtml: "$/GB-month" },
      { label: "Memory per agent", valueHtml: "Configurable" },
      { label: "Disk per agent", valueHtml: "Configurable" },
      { label: "Active agents", valueHtml: "Higher limits" },
      { label: "Apps / envs / components", valueHtml: "Higher limits" },
      { label: "Support", valueHtml: "Community + email" },
    ],
    availability: "Launching in the coming weeks",
    cta: {
      label: "Get notified",
      href: "mailto:hello@golem.cloud?subject=Notify%20me%20about%20Golem%20Cloud%20Paid",
      variant: "primary",
    },
  },
];

export const headroomCallout = {
  bodyHtml: `<strong>Need more?</strong> Memory and disk per agent raise up to your plan's ceiling — self-service with <code>golem account limits</code>. Higher concurrency or project counts are available on request. Email <a href="mailto:sales@golem.cloud?subject=Golem%20Cloud%20Paid%20—%20headroom%20request">sales@golem.cloud</a>.`,
};

// =============================================================================
// Section 4 — On-Prem
// =============================================================================

export const onPrem = {
  eyebrow: "Golem Cloud — On-Prem · Coming soon",
  heading: "Run Golem Cloud inside your own cloud.",
  ledeHtml: `<strong>Golem Cloud On-Prem is the software we run for managed customers</strong>, packaged for your own Kubernetes cluster, on any cloud or on-premises, under an annual license. It is coming soon — talk to us about early access.`,
  includedSubhead: "What's planned",
  included: [
    {
      html: `<strong>Golem Kubernetes Operator</strong> — deploy, scale, and roll out Golem clusters declaratively`,
    },
    {
      html: `<strong>Prebuilt dashboards</strong> for Golem's OpenTelemetry and Prometheus signals — the observability stack we run in production`,
    },
    {
      html: `<strong>Operational tooling</strong> — health monitoring, audit log inspection, deployment lifecycle`,
    },
    { html: `<strong>Placement controls</strong> — how agents are distributed and when they run` },
  ],
  frameText: `The open-source edition is the full platform. Cloud is the version we run for you. On-Prem is the same software and operations stack, packaged for your cloud.`,
  audienceHtml: `<strong>Who it's for:</strong> teams large enough to want Golem inside their own cloud — whether that's because of regulation, sovereignty, or because your existing infrastructure runs on GCP or Azure and you want Golem to run there alongside it.`,
  meta: [
    { label: "Deployment", value: "Kubernetes" },
    { label: "Targets", value: "AWS, GCP, Azure, on-prem" },
    { label: "License", value: "Annual" },
    { label: "Availability", value: "Coming soon" },
  ],
  cta: {
    label: "Talk to sales",
    href: "mailto:sales@golem.cloud?subject=Golem%20Cloud%20On-Prem%20inquiry",
  },
};

// =============================================================================
// Section 5 — Comparison table
// =============================================================================

export interface CompareCell {
  text?: string;
  html?: string;
  kind?: "check" | "x";
}

export const comparisonTable = {
  heading: "At a glance",
  columns: [
    "Open source",
    'Golem Cloud<br /><span class="th-sub">(managed)</span>',
    'Golem Cloud<br /><span class="th-sub">On-Prem</span>',
  ],
  rows: [
    {
      label: "Where it runs",
      cells: [
        { text: "Your infrastructure" },
        { text: "Our infrastructure" },
        { text: "Your infrastructure" },
      ],
    },
    {
      label: "Software",
      cells: [
        { text: "Full platform — you operate it" },
        { text: "Managed for you" },
        { html: `Full platform + ops stack` },
      ],
    },
    {
      label: "Licensing",
      cells: [
        { text: "BUSL‑1.1 → Apache‑2.0" },
        { text: "Hosted service" },
        { text: "Annual commercial license" },
      ],
    },
    {
      label: "Accounts, usage metering, plan limits",
      cells: [
        { html: `✓ <em>(metering off by default)</em>` },
        { kind: "check" },
        { kind: "check" },
      ],
    },
    {
      label: "OpenTelemetry export & Prometheus metrics",
      cells: [{ kind: "check" }, { kind: "check" }, { kind: "check" }],
    },
    {
      label: "Kubernetes Operator",
      cells: [{ kind: "x" }, { kind: "check" }, { kind: "check" }],
    },
    {
      label: "Prebuilt dashboards & ops stack",
      cells: [{ kind: "x" }, { kind: "check" }, { kind: "check" }],
    },
    {
      label: "Pricing model",
      cells: [
        { text: "Free" },
        { text: "Usage-metered: GCU + memory + storage" },
        { text: "Annual license" },
      ],
    },
    {
      label: "Plan tiers",
      cells: [{ text: "—" }, { text: "Free + Paid" }, { text: "Annual" }],
    },
    {
      label: "Support",
      cells: [
        { text: "Ziverge (partner)" },
        { text: "Included by plan" },
        { text: "Included by license" },
      ],
    },
    {
      label: "Availability",
      cells: [
        { text: "Now" },
        { text: "Preview now; paid in the coming weeks" },
        { text: "Coming soon" },
      ],
    },
  ] as { label: string; cells: CompareCell[] }[],
};

// =============================================================================
// Section 6 — Ziverge partner callout
// =============================================================================

export const ziverge = {
  label: "Partner",
  heading: "Looking for support on open source Golem?",
  bodyHtml: `<strong>Ziverge is our exclusive partner</strong> for commercial support, bug fixes, and custom features on the open source edition. They provide direct engineering access for teams running Golem in production.`,
  ctaLabel: "Visit ziverge.com →",
  ctaHref: "https://ziverge.com",
};

// =============================================================================
// Section 7 — FAQ
// =============================================================================

export const faq = {
  heading: "Frequently asked questions",
  items: [
    {
      q: "Is Golem Cloud available today?",
      aHtml: `Yes — Golem Cloud is in <a href="/legal#preview">Developer Preview</a>: free to use, with no SLAs or data-retention guarantees. Paid plans launch in the coming weeks.`,
    },
    {
      q: "What can I do during the Developer Preview?",
      aHtml: `Evaluate, experiment, prototype. The Preview is not intended for production workloads.`,
    },
    {
      q: "When will the Paid tier launch?",
      aHtml: `In the coming weeks. <a href="mailto:hello@golem.cloud?subject=Notify%20me%20about%20Golem%20Cloud%20Paid">Email us</a> and we'll let you know the moment it's live.`,
    },
    {
      q: "How do I see my usage?",
      aHtml: `Run <code>golem account usage show</code> for the current month, <code>golem account usage history</code> for past months, and <code>golem account limits show</code> for your limits. The same data is available from the REST API.`,
    },
    {
      q: "How is compute measured?",
      aHtml: `In <em>Golem Compute Units</em> (GCU). One GCU represents a fixed amount of WebAssembly execution work. Because the unit is deterministic, the same workload produces the same GCU on any machine.`,
    },
    {
      q: "Do idle or suspended agents cost me money?",
      aHtml: `No. Memory and storage are metered only while an agent is working; idle and suspended agents accrue nothing. No traffic, no bill.`,
    },
    {
      q: "What happens if I hit a limit?",
      aHtml: `It depends on the limit. Memory and disk per agent are hard caps. When the monthly compute allowance runs out, durable agents pause. Requests beyond the active-agent limit queue rather than fail. Creating apps, environments, or components beyond the limit is rejected. See <em>Limits and fair use</em> above.`,
    },
    {
      q: "What's the difference between quotas and plan limits?",
      aHtml: `Plan limits are set by your plan and protect the platform. Application quotas are yours: you define resources — API calls, LLM tokens, anything — with rate, capacity, or concurrency limits, and choose whether Golem throttles, rejects, or terminates on overage.`,
    },
    {
      q: "Why is there a free-tier compute allowance?",
      aHtml: `Because compute runs on our infrastructure. The runtime is free; what you pay for is what we run for you.`,
    },
    {
      q: "Why does Golem charge for storage?",
      aHtml: `Every agent has its own durable filesystem. Storage is metered for the files your agents keep on disk while they're working.`,
    },
    {
      q: "What will Golem Cloud On-Prem add over the open source edition?",
      aHtml: `A Kubernetes Operator, prebuilt dashboards, and operational tooling — the same package we use to run Golem Cloud ourselves. On-Prem is coming soon.`,
    },
    {
      q: "Can I run Golem in my own cloud without On-Prem?",
      aHtml: `Yes — the open source edition is the full platform, including accounts, metering, and plan limits. You'd be responsible for your own deployment, monitoring, and operational tooling.`,
    },
    {
      q: "Who provides support for the open source edition?",
      aHtml: `<a href="https://ziverge.com" target="_blank" rel="noopener">Ziverge</a>, our exclusive partner for commercial OSS support.`,
    },
  ],
};
