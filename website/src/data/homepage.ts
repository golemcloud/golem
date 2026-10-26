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
    "Golem runs AI agents, their tools and their artifacts. Each agent runs in a lightweight WebAssembly VM that resumes mid-function after node restarts, rescaling, long waits and crashes. Tool calls run exactly once, under permissions agents can't bypass.",
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
    "Golem runs AI agents, their tools and their artifacts. Each agent runs in a lightweight WebAssembly VM that resumes mid-function after node restarts, rescaling, long waits and crashes. Tool calls run exactly once, under permissions agents can't bypass.",
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
  lead: "Golem runs every agent and every tool call in its own WebAssembly VM. Every call out of a VM goes through Golem, which enforces your permissions and records external interactions so the VM can resume anywhere.",
  alt: "An agent runs on node 1 in its own WebAssembly VM. Its only way out is Golem, which checks every call against your permissions: it allows the call to the Orders API and denies the call to the Admin API. The Orders API result is recorded in the agent's oplog, a log of external interactions stored off the node. After a restart, a crash or an idle wait, Golem restores the agent on node 2. It resumes where it left off, with its memory, call stack, files and environment restored, and the Orders API is not called again: the recorded result is used.",
  vm: { name: "Agent", where: "node 1", sub: "in its own WebAssembly VM" },
  golem: { name: "Golem", sub: "the VM's only way out" },
  calls: [
    { status: "allowed", target: "Orders API" },
    { status: "denied", target: "Admin API" },
  ],
  recorded: "recorded",
  oplog: {
    title: "Oplog",
    sub: "external interactions, stored off the node",
    entries: [{ name: "Orders API", value: "result" }],
  },
  rebuilt: "restored after a restart, a crash or an idle wait",
  restored: {
    name: "Agent, recovered",
    where: "node 2",
    state: "Resumes where it left off",
    sub: "memory, call stack, files and environment restored",
    reused: "Orders API not called again: the recorded result is used",
  },
  caption: "You don't write save or checkpoint code, design state tables or run a workflow engine.",
  explanations: [
    {
      title: "Lightweight",
      body: "WebAssembly, not Linux: each VM has its own memory, file system and environment, with no guest OS. Code can be tens of kilobytes; a VM starts in as little as 2 ms.",
    },
    {
      title: "Durable",
      body: "A waiting VM is suspended, using no CPU or memory, for minutes or months. After a crash or node restart it's restored where it left off, with its files, without repeating completed calls.",
    },
    {
      title: "Secure",
      body: "A VM reaches its own files, the network, secrets and other VMs only through Golem's permission checks, which apply to AI-generated code just as to yours.",
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
  // Feature id: names the folder of this bullet's snippets.
  id: string;
  title: string;
  body: string;
}

export const codeLanguages = [
  { id: "ts", label: "TypeScript", shiki: "typescript" },
  { id: "effect", label: "Effect", shiki: "typescript" },
  { id: "rust", label: "Rust", shiki: "rust" },
  { id: "scala", label: "Scala", shiki: "scala" },
  { id: "moonbit", label: "MoonBit", shiki: "moonbit" },
  { id: "go", label: "Go", shiki: "go" },
] as const;

export type CodeLanguage = (typeof codeLanguages)[number]["id"];

export interface CodePane {
  filename: string;
  source: string;
}

// Snippets live in website/snippets/<chapter>/<feature>/<language>/<file>,
// or <chapter>/<feature>/<file> for a file every language shares. Each
// chapter's first point, "overview", is selected until the reader picks
// another. A line ending in " //@" (" #@" in YAML) is highlighted while its
// snippet shows; the renderer strips the marker.
export type ChapterSnippets = Record<string, Partial<Record<CodeLanguage, CodePane>>>;

const snippetFiles = import.meta.glob<string>("../../snippets/**/*", {
  query: "?raw",
  import: "default",
  eager: true,
});

function snippets(chapter: string): ChapterSnippets {
  const out: ChapterSnippets = {};
  for (const [key, source] of Object.entries(snippetFiles)) {
    const [name, feature, ...rest] = key.slice("../../snippets/".length).split("/");
    if (name !== chapter) continue;
    const filename = rest.pop()!;
    const pane = { filename, source: source.replace(/\n+$/, "") };
    const byLang = (out[feature] ??= {});
    if (rest.length) byLang[rest[0] as CodeLanguage] = pane;
    else for (const l of codeLanguages) byLang[l.id] ??= pane;
  }
  return out;
}

export const agentsChapter = {
  eyebrow: "Agents",
  heading: "Agents are your own typed code, not prompts",
  snippets: snippets("agents"),
  // Shown instead of a snippet in languages that lack the feature.
  missing: {
    sqlite:
      "Scala, MoonBit and Go agents each have their own durable file system too, but no supported SQLite library.",
  } as Record<string, string>,
  items: [
    {
      id: "overview",
      title: "Same guarantees and primitives in six SDKs",
      body: "TypeScript, Effect, Rust, Scala, MoonBit and Go agents resume mid-function with memory and files after a crash.",
    },
    {
      id: "sqlite",
      title: "Per-agent databases",
      body: "Each agent's durable file system can hold an embedded database such as SQLite: no server to run or connect to.",
    },
    {
      id: "models",
      title: "Any model, with your libraries",
      body: "Model calls can use OpenAI or Anthropic SDKs, Vercel AI SDK, TanStack AI, Effect AI or a provider's own HTTP API.",
    },
    {
      id: "team",
      title: "Agents working together",
      body: "Agents make typed cross-language calls, wait or move on, and discover what other agents and tools offer.",
    },
    {
      id: "waits",
      title: "Awaitable webhooks, timers, schedules",
      body: "Agents await approval and payment webhooks. Sleeps and scheduled calls fire on time across restarts, at no idle cost.",
    },
    {
      id: "stream",
      title: "Resumable streaming endpoints",
      body: "Clients that reload or reconnect pick up an agent's streamed output where they left off, even after a crash.",
    },
    {
      id: "readonly",
      title: "Cached read-only methods",
      body: "Read-only methods answer repeat calls from a cache, without loading the agent, until a write succeeds.",
    },
  ] as ChapterItem[],
};

export const toolsChapter = {
  eyebrow: "Tools",
  heading: "Every tool call in its own VM, executed exactly once",
  snippets: snippets("tools"),
  items: [
    {
      id: "overview",
      title: "Overview",
      body: "Each call's fresh VM can't read agent memory. Recovery reuses recorded results rather than repeating those calls.",
    },
    {
      id: "custom",
      title: "Tools without a sandbox service",
      body: "Shell, Git, Node.js, npm and TypeScript are bundled and can use the agent's files. Custom tools use any Golem SDK.",
    },
    {
      id: "requests",
      title: "HTTP retries from agents and tools",
      body: "Golem retries interrupted requests with an idempotency key it adds; APIs that honor it, like Stripe, act once.",
    },
    {
      id: "permissions",
      title: "Delegated, revocable permissions",
      body: "One agent lends another and its tools only the orders API for an hour. Revoking it revokes all access passed on.",
    },
    {
      id: "middleware",
      title: "Middleware that agent code can't bypass",
      body: "Hooks on every tool call can block bad arguments, hold refunds for human approval, redact, audit and rate-limit.",
    },
    {
      id: "secrets",
      title: "Secrets",
      body: "Agents pass API keys to tools as opaque handles. Revealing one takes a grant, is recorded and can be barred per tool.",
    },
    {
      id: "publish",
      title: "Serve agents and tools over MCP",
      body: "Calls to MCP servers get tool-call guarantees. Grant other environments and accounts immutable tool releases.",
    },
  ] as ChapterItem[],
};

export const artifactsChapter = {
  eyebrow: "Artifacts",
  heading: "Charts, pages and apps your agents make on request",
  snippets: snippets("artifacts"),
  items: [
    {
      id: "overview",
      title: "Overview",
      body: "Asked for a sales graph, a tool writes a PNG and HTML page in agent files. Golem serves both only when complete.",
    },
    {
      id: "routes",
      title: "Apps for one user's request, no deploy",
      body: "A tool saves the model's small app in agent files. Buttons call endpoints running durable code in the agent's own VM.",
    },
    {
      id: "peruser",
      title: "One agent per user, from the URL",
      body: "Ana's first request to /dashboards/ana creates her agent. Pages and files it makes for her appear under her path.",
    },
    {
      id: "files",
      title: "Images and PDFs as the reply",
      body: "An agent can reply with a PNG or PDF it just made, to a browser over HTTP or a chat assistant over MCP.",
    },
    {
      id: "routers",
      title: "CSV downloads through a router",
      body: "A router matches the path and method, then streams the agent's CSV as a download.",
    },
    {
      id: "signin",
      title: "OAuth2 sign-in with PKCE",
      body: "Login uses Google, GitLab or any OpenID Connect provider. Golem checks its short-lived token before agent code runs.",
    },
    {
      id: "login",
      title: "Only for the person who asked",
      body: "The agent records who asked for each chart. Anyone else, even signed in, gets a 404.",
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
      body: "One binary runs the whole platform on your laptop. Restart it, and your agents are still there.",
    },
    {
      visual: "selfhost",
      title: "Self-hosted",
      body: "Run the full open source platform in production, on your own Docker or Kubernetes infrastructure.",
    },
    {
      visual: "cloud",
      title: "Golem Cloud",
      body: "Deploy to the managed service; we run the servers. Free during the Developer Preview, then usage-based with a $0 tier.",
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
      body: "Rate, capacity and concurrency limits throttle, reject or terminate runaway agents and tools, and can change without a redeploy.",
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
        "Probably not. Each agent's whole execution is durable, and each agent has its own durable file system and can have its own database. Golem Social, a whole social network, runs on Golem with no external database. Keep existing databases where other systems share the data.",
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
        "Workflow engines usually ask you to split work into activities they record. In Golem, ordinary code is durable as it runs, with no restructuring, and each agent also gets its own VM, with a durable file system, and runtime-checked permissions.",
    },
    {
      q: "Does showing an artifact need a build or deploy step?",
      aHtml:
        "No. The agent makes a new chart, page, file or small app for one user's request and shows it to that user. Golem serves it directly from the agent. The agent does not change an existing app and needs no access to your build or deploy infrastructure.",
    },
    {
      q: "Is Golem open source?",
      aHtml:
        "Yes. Golem uses the BUSL-1.1 license, which converts to Apache-2.0 after four years. Self-hosting for production workloads is permitted.",
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
  lead: "Scaffold a durable agent, run it locally, kill the process at any line, and watch it resume from the exact line it had reached.",
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
