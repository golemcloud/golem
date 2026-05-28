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
    "Three ways to run Golem: open source (you run it), Cloud (we run it), or On-Prem (you run it with our tools and support).",
};

export const hero = {
  eyebrow: "Pricing & Commercial Options",
  heading: "Golem Cloud",
  ledeHtml: `<strong>Open source:</strong> you run it (BUSL-1.1, transitioning to Apache-2.0). <strong>Cloud:</strong> we run it. <strong>On-Prem:</strong> you run it with our tools and support.`,
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

export const meteringExplainer = {
  heading: "How Golem Cloud meters usage",
  leadHtml: `Golem Cloud meters four dimensions, each priced independently. <strong>You pay only for what you use</strong> — no monthly base, no minimums.`,
  dimensions: [
    {
      html: `<strong>Compute</strong> — measured in <em>Golem Compute Units</em> (GCU). Same workload, same price — regardless of which node ran it.`,
    },
    {
      html: `<strong>Memory</strong> — GB-seconds while your agent's code is <em>actually executing</em>. Agents waiting for the next event or paused mid-task don't accrue memory charges.`,
    },
    {
      html: `<strong>Durable storage</strong> — GB-month for agents whose state isn't lost when nodes are replaced. When we shelve an inactive agent, you stop paying for it.`,
    },
    {
      html: `<strong>Ephemeral storage</strong> — GB-month for filesystem scratch space during execution.`,
    },
  ],
  footerHtml: `Per-dimension prices finalized at launch. Multi-currency support (USD, GBP, EUR) planned.`,
};

export const pricingPreludeHtml = `<strong>Pay only for what you use.</strong> No monthly base, no minimums. Final per-dimension prices set at launch.`;

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
      { label: "Compute", valueHtml: "1,000 GCU/mo" },
      { label: "RAM per agent", valueHtml: "128 MB <em>(fixed)</em>" },
      { label: "Disk per agent", valueHtml: "256 MB <em>(fixed)</em>" },
      { label: "Concurrent agents", valueHtml: "10" },
      { label: "Apps / envs", valueHtml: "10 / 10" },
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
    priceAmount: "$0",
    priceUnit: "/month base",
    priceTagline: "Pay only for what you use",
    pill: "Coming Soon",
    emphasized: true,
    specs: [
      { label: "Compute", valueHtml: "$/GCU" },
      { label: "Memory", valueHtml: "$/GB-second" },
      { label: "Durable storage", valueHtml: "$/GB-month" },
      { label: "Ephemeral storage", valueHtml: "$/GB-month" },
      { label: "RAM per agent", valueHtml: "Up to 1 GB <em>(via support)</em>" },
      { label: "Disk per agent", valueHtml: "Up to 10 GB <em>(configurable)</em>" },
      { label: "Concurrent agents", valueHtml: "Up to 5,000 <em>(via support)</em>" },
      { label: "Apps / envs", valueHtml: "1,000 / 1,000" },
      { label: "Support", valueHtml: "Community + email" },
    ],
    availability: "Coming Soon",
    cta: {
      label: "Notify me",
      href: "mailto:hello@golem.cloud?subject=Notify%20me%20about%20Golem%20Cloud%20Paid",
      variant: "primary",
    },
  },
];

export const headroomCallout = {
  bodyHtml: `<strong>Need more than the Paid defaults?</strong> RAM per agent, concurrency, app/environment counts, and compute caps raise via support for legitimate workloads — same per-dimension prices, no separate tier. Email <a href="mailto:sales@golem.cloud?subject=Golem%20Cloud%20Paid%20—%20headroom%20request">sales@golem.cloud</a>.`,
};

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
      html: `<strong>Operational tooling</strong> — health monitoring, audit log inspection, deployment lifecycle`,
    },
    { html: `<em>Coming soon:</em> controls for how agents are distributed and when they run` },
  ],
  frameText: `The OSS edition is the runtime. Cloud is the version we run for you. On-Prem is the same software, packaged for your cloud.`,
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
        { text: "Usage-metered: GCU + memory + storage" },
        { text: "Annual license" },
      ],
    },
    {
      label: "Plan tiers",
      cells: [
        { text: "—" },
        { text: "Free + Paid" },
        { text: "Annual" },
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
      q: "When will the Paid tier launch?",
      aHtml: `Coming soon — sign up for the waitlist and we'll notify you.`,
    },
    {
      q: "How is compute measured?",
      aHtml: `In <em>Golem Compute Units</em> (GCU). One GCU represents a fixed amount of WebAssembly execution work. Because the unit is deterministic, the same workload produces the same GCU on any machine.`,
    },
    {
      q: "Do idle or suspended agents cost me money?",
      aHtml: `No. Memory is billed only while your agent's code is actively executing. Durable-storage metering stops when we shelve an inactive agent; ephemeral storage stops at the end of each invocation. No traffic, no bill.`,
    },
    {
      q: "What happens if I hit a limit?",
      aHtml: `Concurrent-agent and app/environment caps reject the request at allocation time. Compute, memory, and storage are usage-metered without hard caps; raise the per-agent ceilings by contacting support if your workload outgrows the default.`,
    },
    {
      q: "Why is the free tier capped at 1,000 GCU/month?",
      aHtml: `Because compute runs on our infrastructure, unlike providers that host you on someone else's compute. The runtime is free; what you pay for is what we run for you.`,
    },
    {
      q: "Why does Golem charge for storage when other runtimes don't?",
      aHtml: `Golem's durability depends on a complete history of every action — that's how it recovers when a node fails. We charge for the storage that makes that history possible.`,
    },
    {
      q: "Will invoices come in my local currency?",
      aHtml: `At launch, USD, GBP, and EUR are planned. Other currencies on request.`,
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
