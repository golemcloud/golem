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
    "Golem Cloud is the operations stack we built around the open-source Golem runtime. We host it for you, or you host it yourself.",
};

export const hero = {
  eyebrow: "Pricing & Commercial Options",
  heading: "Golem Cloud",
  ledeHtml: `Golem is open source and free forever (<strong>BUSL-1.1, transitioning to Apache-2.0</strong>). Golem Cloud is the operations stack we built around it — we host it for you, or you host it yourself.`,
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
      heading: "Open source. Free forever.",
      body: "Run the durable agent runtime yourself. BUSL-1.1 today, transitioning to Apache-2.0.",
      ctaLabel: "GitHub →",
      ctaHref: "https://github.com/golemcloud/golem",
    },
    {
      eyebrow: "Managed for you",
      heading: "Golem Cloud",
      body: "Developer Preview today (free, no guarantees). Paid tiers with full SLAs and 90-day retention coming soon.",
      ctaLabel: "Get started →",
      ctaHref: "https://learn.golem.cloud/quickstart",
      pill: "Coming Soon",
      emphasized: true,
    },
    {
      eyebrow: "Inside your cloud",
      heading: "Golem Cloud On-Prem",
      body: "Same software, you host it. Annual license. Runs on AWS, GCP, Azure, or on-premises. Available now.",
      ctaLabel: "Talk to sales →",
      ctaHref: "mailto:sales@golem.cloud",
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

export const previewBanner = {
  pill: "Free during Preview",
  bodyHtml: `<strong>Golem Cloud is currently in Developer Preview</strong> — free to use, AS-IS, with no SLAs or data-retention guarantees. Preview workload data is routinely wiped. The Preview is intended for evaluation, experimentation, and prototyping — not production. Paid tiers with full guarantees and 90-day retention are coming soon. <a href="/legal#preview">See legal →</a>`,
};

export const agentSecondCallout = {
  heading: "What's an agent-second?",
  leadHtml: `An agent-second is one second of a single agent in memory, holding its tier's allotment of RAM. RAM-per-agent defaults to:`,
  tiers: [
    {
      html: `<strong>Free:</strong> 128 MB per agent (fixed — cannot be exceeded)`,
    },
    {
      html: `<strong>Standard:</strong> 512 MB per agent (overage available for agents needing more)`,
    },
    {
      html: `<strong>Custom:</strong> up to 2.1 GB per agent by default; configurable`,
    },
  ],
  footerHtml: `An idle agent in memory continues to consume agent-seconds. An agent that's been <em>suspended</em> — oplog-persisted with no in-memory state — does not. Agents requesting more RAM than their tier default incur RAM overage charges per GB-second of excess. Storage costs continue for the persisted oplog regardless of agent state.`,
};

export const pricingPreludeHtml = `<strong>Pricing is tentative</strong> — final numbers at launch. Unlike providers that host you on someone else's compute, your code runs on our infrastructure. The free tier is small for that reason.`;

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
  priceTalk?: boolean;
  pill?: string;
  emphasized?: boolean;
  specs: PricingSpec[];
  overagesHtml?: string;
  availability: string;
  cta: PricingCta;
}

export const pricingCards: PricingCard[] = [
  {
    name: "Free",
    priceAmount: "$0",
    priceUnit: "/month",
    specs: [
      { label: "RAM per agent", valueHtml: "128 MB <em>(fixed)</em>" },
      { label: "Agent-seconds", valueHtml: "500,000/mo" },
      { label: "Storage", valueHtml: "1 GB" },
      { label: "Retention", valueHtml: "7 days" },
    ],
    overagesHtml: "—",
    availability: "Developer Preview",
    cta: {
      label: "Get started",
      href: "https://learn.golem.cloud/quickstart",
      variant: "secondary",
    },
  },
  {
    name: "Standard",
    priceAmount: "$20",
    priceUnit: "/month",
    pill: "Coming Soon",
    emphasized: true,
    specs: [
      { label: "RAM per agent", valueHtml: "512 MB" },
      { label: "Agent-seconds", valueHtml: "2,000,000/mo" },
      { label: "Storage", valueHtml: "25 GB" },
      { label: "Retention", valueHtml: "30 days <em>(up to 90)</em>" },
    ],
    overagesHtml: `<span class="overage-line"><code>$15</code> per million agent-s</span><span class="overage-line"><code>$30</code> per million GB-s excess RAM</span><span class="overage-line"><code>$1</code> per GB-month storage</span>`,
    availability: "Coming Soon",
    cta: {
      label: "Notify me",
      href: "mailto:hello@golem.cloud?subject=Notify%20me%20about%20Golem%20Cloud%20Standard",
      variant: "primary",
    },
  },
  {
    name: "Custom",
    priceAmount: "Talk to us",
    priceTalk: true,
    specs: [
      { label: "RAM per agent", valueHtml: "Up to 2.1 GB <em>(configurable)</em>" },
      { label: "Agent-seconds", valueHtml: "Tunable" },
      { label: "Storage", valueHtml: "Tunable" },
      { label: "Retention", valueHtml: "Unlimited" },
    ],
    overagesHtml: "Custom — every axis tunable",
    availability: "Coming Soon",
    cta: { label: "Talk to sales", href: "mailto:sales@golem.cloud", variant: "secondary" },
  },
];

// Support row is identical for all three tiers and rendered manually
// in markup since it doesn't need a value variant.
export const supportRow: Record<string, string> = {
  Free: "Community",
  Standard: "Community + email",
  Custom: "Dedicated, SLAs",
};

export const retentionNote = `On the paid Standard tier, oplog history is retained for up to 90 days, configurable per workspace. Retained oplog is compressed and stored to S3 — your storage line covers both active and archival.`;

// =============================================================================
// Section 4 — On-Prem
// =============================================================================

export const onPrem = {
  eyebrow: "Golem Cloud — On-Prem",
  heading: "Run Golem Cloud inside your own cloud.",
  ledeHtml: `<strong>Golem Cloud On-Prem is the identical software we run for managed customers</strong> — only you run it yourself, in your own Kubernetes cluster, on any cloud or on-premises. Licensed annually.`,
  includedSubhead: "What's included",
  included: [
    {
      html: `<strong>Golem Kubernetes Operator</strong> — deploy, scale, and roll out Golem clusters declaratively`,
    },
    {
      html: `<strong>Prebuilt OpenTelemetry exporters + dashboards</strong> — the observability stack we run in production`,
    },
    {
      html: `<strong>Operational tooling</strong> — health monitoring, oplog inspection, deployment lifecycle`,
    },
    { html: `<em>Coming soon:</em> agent rebalancing and scheduling optimisations` },
  ],
  frameText: `The OSS edition is the runtime. Golem Cloud is the operations stack we built around it. On-Prem is the same operations stack, packaged for your cloud.`,
  audienceHtml: `<strong>Who it's for:</strong> teams large enough to want Golem inside their own cloud — whether that's because of regulation, sovereignty, or because your existing infrastructure runs on GCP or Azure and you want Golem to run there alongside it.`,
  meta: [
    { label: "Deployment", value: "Kubernetes" },
    { label: "Targets", value: "AWS, GCP, Azure, on-prem" },
    { label: "License", value: "Annual" },
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
    "Free OSS",
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
        { text: "OSS runtime only" },
        { text: "Runtime + ops stack" },
        { html: `Runtime + ops stack <em>(identical)</em>` },
      ],
    },
    {
      label: "Licensing",
      cells: [
        { text: "BUSL-1.1 → Apache-2.0" },
        { text: "Hosted service" },
        { text: "Annual commercial license" },
      ],
    },
    {
      label: "Kubernetes Operator",
      cells: [{ kind: "x" }, { kind: "check" }, { kind: "check" }],
    },
    {
      label: "Prebuilt OTel + dashboards",
      cells: [{ kind: "x" }, { kind: "check" }, { kind: "check" }],
    },
    {
      label: "Operational tooling",
      cells: [{ kind: "x" }, { kind: "check" }, { kind: "check" }],
    },
    {
      label: "Pricing model",
      cells: [
        { text: "Free" },
        { text: "Agent-seconds + RAM + storage" },
        { text: "Annual license" },
      ],
    },
    {
      label: "Retention",
      cells: [
        { text: "Your responsibility" },
        { text: "90 days configurable" },
        { text: "Your responsibility" },
      ],
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
      cells: [{ text: "Now" }, { text: "Preview today; paid coming soon" }, { text: "Now" }],
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
      aHtml: `Yes — Golem Cloud is currently in <a href="/legal#preview">Developer Preview</a>: free to use, with no SLAs or data-retention guarantees. Paid tiers with full guarantees are coming soon.`,
    },
    {
      q: "What can I do during the Developer Preview?",
      aHtml: `Evaluate, experiment, prototype. The Preview is not intended for production workloads.`,
    },
    {
      q: "When will paid tiers launch?",
      aHtml: `Coming soon — sign up for the waitlist and we'll notify you.`,
    },
    {
      q: "Why is the free tier on paid Cloud going to be small?",
      aHtml: `Because compute runs on our infrastructure, unlike providers that host you on someone else's compute. The runtime is free; what you pay for is what we run for you.`,
    },
    {
      q: "What happens if I exceed my plan?",
      aHtml: `Overage fees on agent-seconds, RAM, and storage. Serious workloads should move to Custom.`,
    },
    {
      q: "What's the retention policy?",
      aHtml: `90 days configurable on managed Cloud (Standard tier). Custom is unlimited. We compress and store retained oplog to S3 under your storage line.`,
    },
    {
      q: "Why does Golem charge for storage when other runtimes don't?",
      aHtml: `Golem's durability depends on a persistent oplog of every effect. The oplog <em>is</em> the recovery mechanism — so we charge for the storage that makes byte-identical replay possible. Retention is configurable for that reason.`,
    },
    {
      q: "What's in Golem Cloud On-Prem that's NOT in the open source edition?",
      aHtml: `Kubernetes Operator, OpenTelemetry exporters, prebuilt dashboards, operational tooling — the same package we use to run Golem Cloud ourselves.`,
    },
    {
      q: "Can I run Golem in my own cloud without buying the On-Prem license?",
      aHtml: `Yes — the OSS runtime is full-featured. You'd be responsible for your own deployment, monitoring, and operational tooling.`,
    },
    {
      q: "Who provides support for the open source edition?",
      aHtml: `<a href="https://ziverge.com" target="_blank" rel="noopener">Ziverge</a>, our exclusive partner for commercial OSS support.`,
    },
  ],
};
