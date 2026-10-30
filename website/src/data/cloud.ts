// Pricing-page copy for /cloud. Layout lives in cloud.astro.
import { urls } from "./site";

const workloadBody = `I'd like to discuss a Golem Cloud deployment.\n\nWorkload and expected volume:\nTarget launch date:\nMeasured compute, memory and filesystem usage (if available):\nPeak capacity and retention needs:\nSupport and deployment requirements:\n`;
export const contact = {
  label: "Discuss your workload",
  href: `mailto:sales@golem.cloud?subject=Planning%20a%20Golem%20Cloud%20workload&body=${encodeURIComponent(workloadBody)}`,
};
export const meta = {
  title: "Golem Cloud pricing",
  description:
    "Try managed Golem Cloud free during Developer Preview. Explore the planned resource pricing model and discuss your production workload.",
};
export const hero = {
  eyebrow: "Pricing",
  heading: "Golem Cloud pricing",
  lead: "Run your agents and tools on infrastructure we manage. Start with the free Developer Preview.",
};
export const offers = [
  {
    id: "preview",
    badge: "Available now",
    name: "Developer Preview",
    price: "Free",
    priceNote: "during Developer Preview",
    description: "For evaluating Golem, trying integrations, and building prototypes.",
    features: [
      "Managed execution for your agents and tools",
      "Deploy with the Golem CLI",
      "Community help on Discord",
    ],
    note: "Resource limits apply. Contact us to check capacity for your workload.",
    cta: { label: "Start building", href: urls.quickstart },
    emphasized: true,
  },
  {
    id: "paid",
    badge: "Planned",
    name: "Usage-based Cloud",
    price: "Rates to come",
    priceNote: "paid service is not available yet",
    description:
      "Planning a production deployment? Talk with us about your workload and requirements.",
    features: [
      "Planned pricing for compute, memory, and filesystem usage",
      "Launch date and commercial terms to be announced",
      "Discuss capacity, deployment, and support needs",
    ],
    note: "A dollar estimate needs the rates and terms for paid Cloud.",
    cta: contact,
    emphasized: false,
  },
];
export const previewNotice = {
  heading: "For prototypes, not production",
  body: "No SLA or data-retention guarantee. Workload data may be deleted.",
  link: { label: "Preview terms", href: "/legal#preview" },
};
export const resources = {
  eyebrow: "The planned pricing model",
  heading: "Four resources. Measured separately.",
  lead: "Golem 1.6 measures execution work, active memory, and active filesystem space. These are the resource dimensions planned for paid Cloud; rates are not yet published.",
  items: [
    {
      name: "Compute",
      unit: "GCU",
      description:
        "WebAssembly execution work. One Golem Compute Unit is one million fuel units—not a request count or a model token.",
    },
    {
      name: "Active memory",
      unit: "GiB-seconds",
      description: "Allocated agent memory multiplied by the time the agent is active.",
    },
    {
      name: "Active durable filesystem",
      unit: "GiB-months",
      description:
        "Allocated filesystem space for durable agents, accumulated while they are active.",
    },
    {
      name: "Active ephemeral filesystem",
      unit: "GiB-months",
      description:
        "Allocated scratch space for ephemeral agents, accumulated while they are active.",
    },
  ],
  states: [
    {
      label: "While working or waiting actively",
      body: "Execution consumes compute. Allocated memory and filesystem space continue to accrue usage while an agent remains active, including during an external wait.",
    },
    {
      label: "While idle or suspended",
      body: "These four resource meters stop accruing. An agent’s age is not its active time. This does not promise unlimited data retention.",
    },
  ],
};
export const budget = {
  eyebrow: "Plan your workload",
  heading: "Start with usage, then work out cost.",
  lead: "An agent run has no fixed resource cost. Measure a representative workload before extrapolating to a month of traffic.",
  steps: [
    {
      number: "01",
      title: "Try a representative workload",
      body: "Exercise the code paths and payload sizes you expect to run. Separate active work from time spent suspended.",
    },
    {
      number: "02",
      title: "Gather the quantities",
      body: "Record GCU, active memory, and durable and ephemeral filesystem usage. Include expected monthly volume and peak capacity.",
    },
    {
      number: "03",
      title: "Discuss the production requirements",
      body: "Share your measurements, launch timing, retention needs, and support or private-deployment requirements. Rates and paid availability remain unannounced.",
    },
  ],
  example: {
    heading: "Memory usage example",
    calculation: "256 MiB × 10,000 active seconds = 2,500 GiB-seconds",
    body: "For an agent holding 256 MiB throughout that active time. Measure compute separately in GCU; there is no fixed conversion from request count or elapsed time.",
  },
  externalCosts:
    "Golem Cloud runs your agent code and tools. Budget separately for model APIs and other services billed through your provider accounts.",
  cta: contact,
  secondary: { label: "Ask in the community", href: urls.discord },
};
export const deployment = {
  heading: "Need to run Golem yourself?",
  options: [
    {
      title: "Self-host the runtime",
      body: "Operate Golem on your own infrastructure. You supply the compute, storage, and operations. The repository contains the software license and setup instructions.",
      link: { label: "View on GitHub", href: urls.github },
    },
    {
      title: "Plan a private deployment",
      body: "The packaged Cloud On-Prem offer is planned. Discuss your cloud, Kubernetes, support, and security requirements with us.",
      link: {
        label: "Discuss private deployment",
        href: "mailto:sales@golem.cloud?subject=Private%20Golem%20deployment",
      },
    },
  ],
};
export const faq = {
  heading: "Pricing questions",
  items: [
    {
      question: "Is the Preview a permanent free tier?",
      answer:
        "No. The managed service is free during Developer Preview for evaluation and prototyping. A permanent free production allowance has not been announced. Paid availability, rates, and included quantities will be published when defined.",
    },
    {
      question: "What happens when I reach a resource limit?",
      answer:
        "The consequence depends on the limit. In Golem 1.6, configured monthly compute or call limits can suspend durable agents and fail ephemeral execution. Per-agent memory or managed-filesystem limits can reject allocations or writes. Account limits can reject creation of additional resources. Contact us to confirm the limits that apply to your Cloud workload.",
    },
    {
      question: "Can resource limits set a maximum monthly bill?",
      answer:
        "No. Resource limits and application quotas are not a whole-bill spending cap. In the 1.6 metering model, monthly memory and filesystem usage are measured without a corresponding monthly runtime cap. Application quotas govern operations your code explicitly reserves; they do not automatically cap model-provider invoices.",
    },
    {
      question: "How do the memory and filesystem units work?",
      answer:
        "A GiB is 1,073,741,824 bytes. Memory is allocated GiB multiplied by active seconds. Filesystem usage is allocated space multiplied by active time, normalized to a 730-hour month. The 1.6 usage API labels these binary-based quantities GB-seconds and GB-month. Usage is reported by UTC calendar month.",
    },
    {
      question: "What do I need before planning a production launch?",
      answer:
        "Paid Golem Cloud is not available yet. Discuss your expected volume, measured resource usage, peak capacity, retention, and support requirements with us. A production budget will need published rates and commercial terms. Preview has no SLA or retention guarantee and must not be used for production workloads.",
    },
  ],
};
export const closing = {
  heading: "Try your agents on Golem Cloud.",
  body: "Deploy a prototype, exercise its real workload, and see how it behaves.",
  primary: { label: "Start building", href: urls.quickstart },
  secondary: contact,
};
