// Homepage copy — single source of truth for all visitor-facing prose on /
//
// Source copy: research/golem-homepage-copy-draft.md.
//
// Editing notes:
//   - Plain strings are rendered as-is.
//   - Fields ending in `Html` are rendered via `set:html`. Use <strong>, <em>, and <code>.

export const meta = {
  title: "Golem: the durable runtime for AI agents",
  description:
    "Golem runs AI agents, their tools and their artifacts: pages, files and live output. Each agent runs in a lightweight WebAssembly VM that resumes mid-function after node restarts, rescaling, long waits and crashes. Tool calls run exactly once, under permissions agents can't bypass.",
};

// =============================================================================
// Hero
// =============================================================================

export type Status = "ok" | "wait" | "fail";

export const hero = {
  // Release pill above the H1. Update with each major release.
  eyebrow: {
    label: "Golem 1.6 is out: see what's new",
    href: "/blog/golem-1-6-agents-tools-and-artifacts",
  },
  heading: "The durable runtime for AI agents",
  subhead:
    "Golem runs AI agents, their tools and their artifacts: pages, files and live output. Each agent runs in a lightweight WebAssembly VM that resumes mid-function after node restarts, rescaling, long waits and crashes. Tool calls run exactly once, under permissions agents can't bypass.",
  ctas: {
    primary: { label: "Start building →", href: "https://learn.golem.cloud/quickstart" },
    secondary: { label: "View on GitHub", href: "https://github.com/golemcloud/golem" },
  },
  artAlt: "",
};

// =============================================================================
// Projects using Golem
// =============================================================================

export interface CustomerLogo {
  // Filename under src/assets/logo/, resolved in the CustomerLogos component
  // so Astro can emit optimized variants at build time.
  filename: string;
  alt: string;
  // Set true for black/dark marks that need inversion on the dark background.
  invertOnDarkBg?: boolean;
}

export const customerLogos = {
  label: "Built on Golem",
  logos: [
    { filename: "ziverge.png", alt: "Ziverge" },
    { filename: "golem-social.png", alt: "Golem Social" },
    { filename: "warpmind.png", alt: "WarpMind" },
    { filename: "seeta-ai-assistant.png", alt: "Seeta AI Assistant" },
    { filename: "golem-journai.png", alt: "JournAI" },
    { filename: "johnethel-lms.png", alt: "Johnethel LMS" },
  ] as CustomerLogo[],
};

// =============================================================================
// Core: the lightweight, durable VM
// =============================================================================

export const coreVm = {
  eyebrow: "The core of Golem",
  heading: "A lightweight, durable VM with permissions agents can't bypass",
  lead: "Golem runs every agent, every tool call and every artifact in its own WebAssembly VM. Every call out of a VM goes through Golem, which checks it against your permissions and records it, so the VM can be rebuilt anywhere.",
  alt: "An agent runs on node 1 in its own WebAssembly VM. Its only way out is Golem, which checks and records every call: it allows the call to the Orders API and denies the call to the Admin API. The Orders API result is recorded in the agent's oplog, a log of every call stored off the node. After a restart, a crash or an idle wait, Golem rebuilds the agent on node 2 from the oplog. It resumes at the exact instruction with its memory, call stack and files restored, and the Orders API is not called again: the recorded result is used.",
  vm: { name: "Agent", where: "node 1", sub: "in its own WebAssembly VM" },
  golem: { name: "Golem", sub: "the VM's only way out" },
  calls: [
    { status: "allowed", target: "Orders API" },
    { status: "denied", target: "Admin API" },
  ],
  recorded: "recorded",
  oplog: {
    title: "Oplog",
    sub: "a log of every call, stored off the node",
    entries: [{ name: "Orders API", value: "result" }],
  },
  rebuilt: "rebuilt after a restart, a crash or an idle wait",
  restored: {
    name: "Agent, rebuilt",
    where: "node 2",
    state: "Resumes at the exact instruction",
    sub: "memory, call stack, files and environment restored",
    reused: "Orders API not called again: the recorded result is used",
  },
  caption: "You don't write save or checkpoint code, design state tables or run a workflow engine.",
  explanations: [
    {
      title: "Lightweight",
      body: "WebAssembly, not Linux: each VM has private memory, files and environment, with no guest OS. Code can be tens of kilobytes; a VM starts in about 2 ms, or under 1 ms at best.",
    },
    {
      title: "Durable",
      body: "A waiting VM is suspended, using no CPU or memory, for minutes or months. After a crash or node restart it's rebuilt at the exact instruction, without repeating completed calls.",
    },
    {
      title: "Secure",
      body: "A VM can't reach files, the network, secrets or other VMs directly. Every call out goes through Golem's permission checks, which no code, including AI-generated code, can bypass.",
    },
  ],
};

// =============================================================================
// Metric bar
// =============================================================================

export const metricBar = {
  metrics: [
    { value: "10,000+", label: "Active agents per node" },
    { value: "2 ms", label: "Agent cold start" },
    { value: "1 MB", label: "Min memory per agent" },
    { value: "0 CPU/RAM", label: "While waiting" },
  ],
};

// =============================================================================
// Tour: agents, tools, artifacts
// =============================================================================

export interface ChapterItem {
  title: string;
  body: string;
}

export interface ChapterCode {
  filename: string;
  code: string;
  highlight: number[];
}

export const agentsChapter = {
  eyebrow: "Agents",
  heading: "Agents are code, not prompts",
  lead: "Each agent's VM keeps its whole execution durable.",
  figure: {
    filename: "support-agent.ts",
    highlight: [11, 12],
    code: `export const SupportImpl = Support.implement({
  init: () => ({ history: [] as Message[] }),
  methods: {
    async handle({ message, orderId }) {
      this.history.push({ role: 'user', content: message })
      // Golem lets lookupOrder reach only the orders API:
      const reply = await llm.run(this.history, [lookupOrder])
      if (reply.refundRequested) {
        const approval = createWebhook()
        await notifyReviewer(approval.getUrl(), reply)
        // waits here for days if needed, using no CPU or memory:
        const { approved } = (await approval).json<Approval>()
        // refunded once if the payment API honors idempotency keys:
        if (approved) await refundOrder(orderId, reply.amount)
      }
      return reply.text
    },
  },
})`,
  } as ChapterCode,
  items: [
    {
      title: "Your language",
      body: "Write agents as ordinary, typed code in TypeScript, Effect, Rust, Scala, MoonBit or Go, with the same guarantees and primitives in each.",
    },
    {
      title: "Awaitable webhooks, timers and schedules",
      body: "Approvals and payment callbacks become promises your code awaits. Sleeps, timers and scheduled calls fire on time across restarts, with no idle cost.",
    },
    {
      title: "Agents working together",
      body: "Typed calls between agents, across languages, right away or scheduled for later. Agents can discover what other agents and tools offer.",
    },
    {
      title: "Any model, with your libraries",
      body: "Call any model through common LLM SDKs. Keep using the OpenAI and Anthropic SDKs, Vercel AI SDK, TanStack AI, Effect AI, Zod and more.",
    },
    {
      title: "Embedded SQLite",
      body: "TypeScript, Effect and Rust agents can each have their own SQLite database, embedded in the agent, with no database server to run or connect to.",
    },
  ] as ChapterItem[],
};

export const toolsChapter = {
  eyebrow: "Tools",
  heading: "Every tool call in its own VM, exactly once",
  lead: "After a node restart or crash, rebuilding a tool call's VM re‑runs its code using recorded results, without repeating completed calls.",
  figure: {
    filename: "refund-tool.ts",
    highlight: [13, 14, 15],
    code: `export const refund = toolDefinition('refund')
  .version('1.0.0')
  .body((body) =>
    body
      .positional('charge', z.string())
      .positional('amount', z.number().int())
      .returns(z.object({ id: z.string(), status: z.string() })),
  )

refund.implement({
  // each call runs in a fresh WebAssembly VM:
  refund: async ({ charge, amount }) => {
    // Golem checks this request against the caller's permissions
    // and adds an idempotency key, so a retry refunds once:
    const res = await fetch('https://api.stripe.com/v1/refunds', {
      method: 'POST',
      body: new URLSearchParams({ charge, amount: String(amount) }),
    })
    return ok(await res.json())
  },
})`,
  } as ChapterCode,
  items: [
    {
      title: "Bundled and custom tools",
      body: "Shell, Git, Node.js, npm and TypeScript come bundled. Build your own tools with any Golem SDK; no separate sandbox service needed.",
    },
    {
      title: "External requests",
      body: "Interrupted external requests retry with idempotency keys Golem adds, taking effect once if the service honors them, as Stripe does.",
    },
    {
      title: "Permissions you can scope and share",
      body: "Give agents or tools narrower permissions, such as the orders API only, for one hour. Revoking a permission revokes every permission delegated from it.",
    },
    {
      title: "Secrets",
      body: "Agents pass API keys to tools as opaque handles. Revealing a value takes a grant and is recorded, and a binding can stop a tool from revealing any.",
    },
    {
      title: "Tool publishing, plus MCP",
      body: "Publish tools as immutable, versioned releases for other accounts. Expose agents and tools over MCP, and call MCP servers with the same guarantees.",
    },
  ] as ChapterItem[],
};

export const artifactsChapter = {
  eyebrow: "Artifacts",
  heading: "Artifacts your agents deploy",
  lead: "Pages, files and live output, with each artifact in its own WebAssembly VM.",
  figure: {
    filename: "site-agent.ts",
    highlight: [4, 5, 6, 7],
    code: `export const SiteAgent = defineAgent({
  name: 'SiteAgent',
  id: { project: z.string() },
  // serve this agent's /public directory at /sites/{project}/:
  http: http.mount('/sites/{project}', {
    exposeFiles: [{ route: '/*', path: '/public/$1' }],
  }),
  methods: { publish: method({ input: {}, returns: z.void() }) },
})

export const SiteAgentImpl = SiteAgent.implement({
  init: () => ({}),
  methods: {
    publish() {
      // copy in the site the agent built; its URLs serve it at once:
      fs.cpSync('/workspace/dist', '/public', { recursive: true })
    },
  },
})`,
  } as ChapterCode,
  items: [
    {
      title: "Routes and files",
      body: "Agents deploy front-ends and back ends under the permissions you grant. HTTP routes are declared in agent code; static files are served directly from storage.",
    },
    {
      title: "Durable streaming",
      body: "Streaming methods become resumable streams: clients that reload or reconnect pick up an agent's output where it left off, even after a crash.",
    },
    {
      title: "Front-end login",
      body: "Single-page front-ends sign users in with OAuth2 and PKCE, using short-lived tokens Golem issues. A mount can turn away callers without a valid login.",
    },
    {
      title: "Read-only methods",
      body: "Methods marked read-only can't change state; the runtime enforces it. Responses are cached until the agent next writes, served without waking it.",
    },
  ] as ChapterItem[],
};

// =============================================================================
// Testimonials
// =============================================================================

export interface Testimonial {
  quote: string;
  name: string;
  role: string;
  company: string;
  // Filename under src/assets/testimonials/; resolved in the component so
  // Astro can optimize the image at build time.
  avatar?: { filename: string; alt: string };
  // Filename under src/assets/logo/; same resolution as above.
  productLogo?: { filename: string; alt: string };
}

export const customerStories = {
  heading: "From developers building on Golem",
  testimonials: [
    {
      quote:
        "Tried Temporal, Dapr, Restate, and Fermyon Spin. Golem gives me what I'd given up on finding: code-level customizability with no-code-level operational simplicity.",
      name: "Samuel Iyeh",
      role: "AI Software Engineer",
      company: "Building Johnethel LMS",
      avatar: { filename: "samuel-iyeh.jpg", alt: "Samuel Iyeh" },
      productLogo: { filename: "johnethel-lms.png", alt: "Johnethel LMS" },
    },
    {
      quote:
        "I tried Akka event sourcing and Step Functions first. What I love about Golem is clean code — durable state and automatic retry built in.",
      name: "Peter Kotula",
      role: "Software Engineer",
      company: "Building Golem Social",
      avatar: { filename: "peter-kotula.jpg", alt: "Peter Kotula" },
      productLogo: { filename: "golem-social.png", alt: "Golem Social" },
    },
    {
      quote:
        "I crashed a running agent mid-test — its .schedule() reminder still fired on time. Golem's durability isn't bolted on; it's the runtime.",
      name: "Rahul Joshi",
      role: "Full-Stack AI Developer",
      company: "Building WarpMind",
      avatar: { filename: "rahul-joshi.jpg", alt: "Rahul Joshi" },
      productLogo: { filename: "warpmind.png", alt: "WarpMind" },
    },
    {
      quote:
        "Like AWS Lambda, but the function has memory across invocations and durable reminders. Durability is invisible — exactly how infrastructure should feel.",
      name: "Seeta Vadali",
      role: "Senior Consultant",
      company: "Building Seeta AI",
      avatar: {
        filename: "seeta-ramayya-vadali.jpg",
        alt: "Seeta Ramayya Vadali",
      },
      productLogo: { filename: "seeta-ai-assistant.png", alt: "Seeta AI Assistant" },
    },
    {
      quote:
        "For JournAI, I modeled log analysis as durable agents. Golem's guarantees around state, retries, and recovery let me focus on the logic, not the infrastructure.",
      name: "Daniele Torelli",
      role: "Senior Software Engineer",
      company: "Building JournAI",
      avatar: { filename: "daniele-torelli.png", alt: "Daniele Torelli" },
      productLogo: { filename: "golem-journai.png", alt: "JournAI" },
    },
  ] as Testimonial[],
};

// =============================================================================
// Deployment
// =============================================================================

export const deployment = {
  eyebrow: "Operations",
  heading: "Run it locally, self-hosted or managed",
  options: [
    {
      visual: "local",
      title: "Local",
      body: "One binary runs the whole platform on your laptop. Stop it, restart it, and your agents are still there.",
    },
    {
      visual: "selfhost",
      title: "Self-hosted",
      body: "Run the full open source platform in production, on your own Docker or Kubernetes infrastructure.",
    },
    {
      visual: "cloud",
      title: "Golem Cloud",
      body: "Deploy to the managed service and let us run it, with no servers of your own. Free during the Developer Preview.",
    },
  ],
  sharedLabel: "In every deployment",
  shared: [
    {
      title: "Observability and audit",
      body: "OpenTelemetry traces and metrics. Each agent's oplog records its external interactions and permission decisions for audit.",
    },
    {
      title: "Live updates and rollback",
      body: "Update running agents after a compatibility check. Roll an agent back to an earlier point, or clone it to try another approach.",
    },
    {
      title: "Quotas",
      body: "Rate, capacity and concurrency limits throttle, reject or stop runaway agents and tools, and can change without a redeploy.",
    },
  ],
};

// =============================================================================
// FAQ
// =============================================================================

export const faq = {
  heading: "Questions developers ask",
  items: [
    {
      q: "Do I still need my database?",
      aHtml:
        "Probably not. Each agent's whole execution is durable, and each agent has its own file system and can have its own database. Golem Social, a whole social network, runs on Golem with no external database. The same holds for your agents and tools, and for the artifacts your agents deploy. Keep existing databases where other systems share the data.",
    },
    {
      q: "Can I keep my agent framework?",
      aHtml:
        "Yes. A framework shapes the agent loop and usually saves the conversation between turns. It runs inside a Golem agent, which keeps the agent's whole execution durable, in-progress tool calls included, and enforces permissions in the runtime.",
    },
    {
      q: "How is this different from an actor platform?",
      aHtml:
        "Actor platforms persist each entity's state. Golem keeps the agent's whole execution durable, including its call stack, files and environment, so it continues in the middle of a call. Tool calls run exactly once, and the runtime enforces permissions.",
    },
    {
      q: "How does it compare with a workflow engine?",
      aHtml:
        "Workflow engines usually ask you to split work into activities they record. In Golem, ordinary code is durable as it runs, with no restructuring, and each agent also gets its own VM and runtime-checked permissions.",
    },
    {
      q: "Is Golem open source?",
      aHtml:
        "Yes. Golem has a BSL license that converts to Apache 2.0, and is available for self-hosting for production workloads.",
    },
    {
      q: "What happens to running agents when I deploy?",
      aHtml: `They stay on their current version until you update them. Golem checks that the new code is compatible before it moves a running agent over. <a href="https://learn.golem.cloud">Read the docs →</a>`,
    },
  ],
};

// =============================================================================
// Final CTA
// =============================================================================

export const finalCta = {
  heading: "Build your first durable agent",
  body: "Install one binary and run the platform locally.",
  ctas: {
    primary: { label: "Start building →", href: "https://learn.golem.cloud/quickstart" },
    secondary: { label: "View on GitHub", href: "https://github.com/golemcloud/golem" },
  },
};

// =============================================================================
// Quickstart (used by /cloud and /use-cases)
// =============================================================================

export const quickstart = {
  // headingLines render with <br> between.
  headingLines: ["Crash your first agent in five minutes.", "Watch it come back."],
  lead: "Scaffold a durable agent, run it locally, kill the process at any line, and watch it resume exactly where it stopped.",
  installCommand: `# Install: download from github.com/golemcloud/golem/releases
# Templates: ts | effect | rust | go | scala | moonbit
golem new --template ts --component-name example:counter --yes my-agent
cd my-agent && golem build
golem repl`,
  primaryCtas: [
    {
      label: "Get started →",
      href: "https://learn.golem.cloud/quickstart",
      variant: "primary" as const,
    },
    { label: "Read the docs", href: "https://learn.golem.cloud", variant: "secondary" as const },
    {
      label: "View on GitHub",
      href: "https://github.com/golemcloud/golem",
      variant: "secondary" as const,
    },
  ],
  secondaryLinks: [
    {
      labelHtml: `Join the <strong>Discord →</strong>`,
      href: "https://discord.com/invite/UjXeH8uG4x",
    },
  ],
};
