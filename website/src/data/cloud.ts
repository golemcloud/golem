// Pricing-page copy for /cloud. Layout lives in cloud.astro.
import { urls } from "./site";

const workloadBody = `I'd like to discuss a Golem Cloud deployment.\n\nWorkload and expected volume:\nTarget launch date:\nMeasured compute, memory and filesystem usage (if available):\nPeak capacity and retention needs:\nSupport and deployment requirements:\n`;
export const contact = {
  label: "Discuss your workload",
  href: `mailto:sales@golem.cloud?subject=Planning%20a%20Golem%20Cloud%20workload&body=${encodeURIComponent(workloadBody)}`,
};
const upgrade = { label: "Upgrade to Pro", href: urls.billing };
export const meta = {
  title: "Golem Cloud pricing",
  description:
    "Golem Cloud has two plans. Free includes small monthly amounts of compute, memory and filesystem. Pro costs $20 a month plus tax, with larger amounts and optional paid overage.",
};
export const hero = {
  eyebrow: "Pricing",
  heading: "Golem Cloud pricing",
  lead: "Run your agents and tools on infrastructure we manage. Start on Free, and move to Pro when your workload needs more.",
};
export const offers = [
  {
    id: "free",
    name: "Free",
    price: "$0",
    period: "a month",
    priceNote: "Every new account starts on Free. It replaces the Developer Preview.",
    description: "Try Golem Cloud with a small monthly amount of each resource.",
    included: [
      { amount: "1 million GCU", label: "of compute" },
      { amount: "About 1.9 GiB-hours", label: "of active memory" },
      { amount: "1 GiB-month", label: "of active durable filesystem" },
      { amount: "1 GiB-month", label: "of active ephemeral filesystem" },
    ],
    runOut:
      "Durable agents pause until the 1st of the next month (UTC). They keep their state and queued calls. Calls to ephemeral agents fail. Nothing is deleted.",
    cta: { label: "Start building", href: urls.quickstart },
    ctaNote: "",
    emphasized: false,
  },
  {
    id: "pro",
    name: "Pro",
    price: "$20",
    period: "a month",
    priceNote:
      "Tax added at checkout. €24 in countries that use the euro and £19 in the UK, tax included.",
    description:
      "Run your agents and tools on Golem Cloud, with a monthly allowance of each resource.",
    included: [
      { amount: "100 million GCU", label: "of compute" },
      { amount: "912.5 GiB-hours", label: "of active memory" },
      { amount: "5 GiB-months", label: "of active durable filesystem" },
      { amount: "5 GiB-months", label: "of active ephemeral filesystem" },
    ],
    runOut:
      "Durable agents pause until the 1st of the next month (UTC). They keep their state and queued calls. Calls to ephemeral agents fail. Nothing is deleted. If the account owner turns on paid overage, agents keep running and the extra usage goes on the next invoice.",
    cta: upgrade,
    ctaNote:
      "Sign in with GitHub at billing.golem.cloud and upgrade. Cancel at any time. Pro runs until the end of the billing period.",
    emphasized: true,
  },
];
export const planNotes = [
  {
    heading: "Custom plans",
    body: "Need more than Pro includes? We set up custom plans by arrangement.",
    link: { label: "sales@golem.cloud", href: contact.href },
  },
  {
    heading: "Payment",
    body: "Polar, our merchant of record, takes the payment and picks the currency from your location at checkout. You pay in EUR in countries that use the euro and in GBP in the UK, with tax included. Everywhere else you pay in USD, with tax added at checkout.",
    link: { label: "Terms of service", href: "/legal" },
  },
];
export const overage = {
  eyebrow: "Pro only",
  heading: "Paid overage",
  lead: "Paid overage is off until the account owner turns it on at billing.golem.cloud. With it on, your agents keep running after an included amount runs out. The extra usage goes on your next monthly invoice.",
  caption: "Paid overage prices, per unit above the included amounts",
  rowHeader: "Per unit above the included amounts",
  currencies: [
    { code: "USD", tax: "tax added" },
    { code: "EUR", tax: "tax included" },
    { code: "GBP", tax: "tax included" },
  ],
  rows: [
    { name: "Compute", unit: "per million GCU", prices: ["$0.0112", "€0.0131", "£0.0103"] },
    { name: "Active memory", unit: "per GiB-hour", prices: ["$0.045", "€0.053", "£0.042"] },
    {
      name: "Active durable filesystem",
      unit: "per GiB-month",
      prices: ["$0.27", "€0.32", "£0.25"],
    },
    {
      name: "Active ephemeral filesystem",
      unit: "per GiB-month",
      prices: ["$0.24", "€0.28", "£0.23"],
    },
  ],
  note: "The four resource names are also the line names on your invoice.",
};
export const resources = {
  eyebrow: "How usage is measured",
  heading: "Four resources. Measured separately.",
  lead: "Each plan includes a monthly amount of four resources. Golem measures execution work, active memory, and active filesystem space, and counts each one against its own amount.",
  items: [
    {
      name: "Compute",
      unit: "GCU",
      description:
        "WebAssembly execution work. One Golem Compute Unit is one million fuel units. It is not a request count or a model token.",
    },
    {
      name: "Active memory",
      unit: "GiB-hours",
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
      body: "These four resource meters stop accruing. An agent's age is not its active time. This does not promise unlimited data retention.",
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
      title: "Compare with a plan",
      body: "Scale the quantities to a month and compare each one with the included amounts. Price what goes above Pro with the overage table, or ask us about a custom plan.",
    },
  ],
  example: {
    heading: "Memory usage example",
    calculation: "256 MiB × 100 active hours = 25 GiB-hours",
    body: "That is one agent holding 256 MiB for 100 hours of active time. Pro's 912.5 GiB-hours cover 36 such agents a month. Measure compute separately in GCU. Request count and elapsed time do not convert to GCU at a fixed rate.",
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
      question: "What happens when an included amount runs out?",
      answer:
        "On both plans, durable agents pause until the 1st of the next month (UTC). They keep their state and queued calls, and calls to ephemeral agents fail. Nothing is deleted. On Pro, the account owner can turn on paid overage instead. Agents then keep running, and the extra usage goes on the next invoice. Per-agent memory and filesystem limits are separate from the monthly amounts. Reaching one can reject an allocation or a write.",
    },
    {
      question: "How do I control what I pay?",
      answer:
        "Free costs nothing. With paid overage off, Pro costs its monthly price and nothing more, because usage stops at the included amounts. With paid overage on, each unit above the included amounts costs the price in the overage table. The account owner turns paid overage on and off at billing.golem.cloud.",
    },
    {
      question: "How do I upgrade to Pro?",
      answer:
        "Sign in with GitHub at billing.golem.cloud and upgrade. Polar runs the checkout. You can cancel at any time, and Pro runs until the end of the billing period.",
    },
    {
      question: "How do the memory and filesystem units work?",
      answer:
        "A GiB is 1,073,741,824 bytes. Memory usage is allocated GiB multiplied by active hours. Filesystem usage is allocated space multiplied by active time, normalized to a 730-hour month, so a GiB-month is 1 GiB for 730 hours. The included amounts reset on the 1st of each month (UTC).",
    },
  ],
};
export const closing = {
  heading: "Try your agents on Golem Cloud.",
  body: "Start on Free. Deploy a prototype, run its real workload, and see what it uses.",
  primary: { label: "Start building", href: urls.quickstart },
  secondary: upgrade,
};
