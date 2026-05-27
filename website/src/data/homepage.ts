// Homepage copy — single source of truth for all visitor-facing prose on /
//
// Editing notes:
//   - Plain strings are rendered as-is.
//   - HTML strings (paragraphs in cards, hero expander, etc.) are rendered via
//     `set:html` in the components. Use <strong>, <em>, <code>, and the marker
//     span for 1.6 asterisks:  <span class="marker">*</span>
//   - The marker is the rose-pink * that links visually to the 1.6 footnote.
//   - When 1.6 ships: search-and-delete <span class="marker">*</span> instances
//     across this file, then remove the footnote section from the page.

// =============================================================================
// SECTION 1 — Hero (LOCKED)
// =============================================================================

export const hero = {
  // headingLines render as <br>-separated lines inside the same <h1>.
  headingLines: ["Agents that never fail.", "Policies that never bend."],
  // expander is HTML (uses <strong>, <br>, <span class="closer">).
  expanderHtml: `The durable agent runtime that <strong>persists state</strong>, <strong>executes tools transactionally</strong>, and <strong>enforces every policy</strong>.<br class="break-on-md" /> <span class="closer">Reliability and trust by construction.</span>`,
  ctas: {
    primary: { label: "Get started →", href: "https://learn.golem.cloud/quickstart" },
    secondary: { label: "View on GitHub", href: "https://github.com/golemcloud/golem" },
  },
  // Right-column hero imagery. Currently a single image imported directly
  // in Hero.astro; only the alt text comes from here.
  images: [{ alt: "Golem thinker — stone figure in contemplation" }],
};

// =============================================================================
// SECTION 2 — Code (the M1 proof)
// =============================================================================

export const codeSection = {
  eyebrow: "Code-first",
  heading: "Agents are code, not prompts.",
  lead: "Typed agents and tools in TypeScript, Rust, Scala, or MoonBit. State persists across failures, tool calls fire exactly once, and your code harnesses the model.",
  defaultLang: "typescript" as const,
  tabs: [
    { id: "typescript", label: "TypeScript", filename: "orders-agent.ts", lang: "typescript" },
    { id: "rust", label: "Rust", filename: "orders_agent.rs", lang: "rust" },
    { id: "scala", label: "Scala", filename: "OrdersAgent.scala", lang: "scala" },
    { id: "moonbit", label: "MoonBit", filename: "orders_agent.mbt", lang: "moonbit" },
  ],
  snippets: {
    typescript: `const Orders = agentDefinition('orders')
  .id({ customerId: z.string() })
  .config(z.object({ systemPrompt: z.string() }))
  .method('handle', m => m
    .input(z.object({ request: z.string(), orderId: z.string() }))
    .returns(z.object({ resolved: z.boolean() })))

export default Orders.implement({
  init: () => ({ history: [] as Message[] }),
  methods: {
    async handle({ request, orderId }) {
      // Durable in-memory state — survives crashes, deploys, host migrations
      this.history.push({ role: 'user', content: request })

      // LLM sees full conversation; system prompt comes from typed config
      const outcome = await llm.run({
        prompt: this.config.systemPrompt, history: this.history,
        tools: [cancelOrder, changeAddress], context: { orderId },
      })
      this.history.push({ role: 'assistant', content: outcome.message })

      // Refunds aren't in the LLM's toolset — agent code gates them via HITL
      if (outcome.needsRefund) {
        const { approved } = await webhooks.awaitApproval(outcome)
        if (approved) {
          // Transactional — refund executes exactly once, even through crashes or restarts
          const result = await refundOrder({ orderId, amount: outcome.refundAmount })
          this.history.push({ role: 'tool', content: JSON.stringify(result) })
        }
      }
      return { resolved: true }
    },
  },
})`,

    rust: `#[derive(ConfigSchema)]
pub struct OrdersConfig { pub system_prompt: String }

#[agent_definition]
pub trait Orders {
    async fn handle(&mut self, request: String, order_id: String) -> bool;
}

struct OrdersImpl { config: Config<OrdersConfig>, history: Vec<Message> }

#[agent_implementation]
impl Orders for OrdersImpl {
    async fn handle(&mut self, request: String, order_id: String) -> bool {
        // Durable in-memory state — survives crashes, deploys, host migrations
        self.history.push(Message::user(request));

        // LLM sees full conversation; system prompt comes from typed config
        let outcome = llm::run(
            &self.config.get().system_prompt, &self.history,
            vec![cancel_order(), change_address()], order_id.clone(),
        ).await;
        self.history.push(Message::assistant(outcome.message.clone()));

        // Refunds aren't in the LLM's toolset — agent code gates them via HITL
        if outcome.needs_refund {
            let approval = create_webhook().await.json::<Approval>();
            if approval.approved {
                // Transactional — refund executes exactly once, even through crashes or restarts
                let result = refund_order(order_id, outcome.refund_amount).await;
                self.history.push(Message::tool(result.to_string()));
            }
        }
        true
    }
}`,

    scala: `final case class OrdersConfig(systemPrompt: String)

@agentDefinition()
trait Orders extends BaseAgent with AgentConfig[OrdersConfig]:
  def handle(request: String, orderId: String): Future[Boolean]

@agentImplementation()
final class OrdersImpl(customerId: String, config: Config[OrdersConfig]) extends Orders:
  private var history: Vector[Message] = Vector.empty

  override def handle(request: String, orderId: String): Future[Boolean] = Future:
    // Durable in-memory state — survives crashes, deploys, host migrations
    history = history :+ Message("user", request)

    // LLM sees full conversation; system prompt comes from typed config
    val outcome = llm.run(
      prompt = config.value.systemPrompt, history = history,
      tools = Seq(cancelOrder, changeAddress), context = Map("orderId" -> orderId)).await
    history = history :+ Message("assistant", outcome.message)

    // Refunds aren't in the LLM's toolset — agent code gates them via HITL
    if outcome.needsRefund then
      val approval = HostApi.createWebhook().await.json[Approval]()
      if approval.approved then
        // Transactional — refund executes exactly once, even through crashes or restarts
        val result = refundOrder(orderId, outcome.refundAmount).await
        history = history :+ Message("tool", result.toString)
    true`,

    moonbit: `#derive.config
pub(all) struct OrdersConfig { system_prompt : String }

#derive.agent
struct Orders {
  config : @config.Config[OrdersConfig]
  mut history : Array[Message]
}

pub fn Orders::handle(self : Self, request : String, order_id : String) -> Bool {
  // Durable in-memory state — survives crashes, deploys, host migrations
  self.history.push({ role: "user", content: request })

  // LLM sees full conversation; system prompt comes from typed config
  let outcome = @llm.run(
    prompt = self.config.value().system_prompt, history = self.history,
    tools = [cancel_order(), change_address()], context = { "order_id": order_id },
  )
  self.history.push({ role: "assistant", content: outcome.message })

  // Refunds aren't in the LLM's toolset — agent code gates them via HITL
  if outcome.needs_refund {
    let approval : Approval = @webhook.create().wait().json()
    if approval.approved {
      // Transactional — refund executes exactly once, even through crashes or restarts
      let result = refund_order(order_id, outcome.refund_amount)
      self.history.push({ role: "tool", content: result.to_json().stringify() })
    }
  }
  true
}`,
  } as Record<string, string>,
};

// =============================================================================
// SECTION 3 — Customer logos (GATED at launch)
// =============================================================================

export interface CustomerLogo {
  kind: "image" | "placeholder";
  // For kind: "image" — filename under src/assets/logo/. Resolved to an
  // imported ImageMetadata in the CustomerLogos component, which lets Astro
  // emit responsive WebP/AVIF at build time.
  filename?: string;
  alt?: string;
  name?: string;
  // Set true for black/dark marks that need inversion to read on the dark
  // logo-bar background. Authored-for-dark logos (white text) leave this off.
  invertOnDarkBg?: boolean;
}

export const customerLogos = {
  label: "Builders shipping on Golem",
  // Real logos use kind: "image"; placeholder text entries use kind: "placeholder".
  // Add invertOnDarkBg: true for black-on-transparent marks.
  showAtLaunch: true,
  placeholders: [
    { kind: "image", filename: "ziverge.png", alt: "Ziverge" },
    { kind: "image", filename: "golem-social.png", alt: "Golem Social" },
    { kind: "image", filename: "warpmind.png", alt: "WarpMind" },
    { kind: "image", filename: "seeta-ai-assistant.png", alt: "Seeta AI Assistant" },
  ] as CustomerLogo[],
};

// =============================================================================
// SECTION 4 — Three commitments + W3 sidebar
// =============================================================================

export interface Commitment {
  id: string;
  icon: "journal" | "exchange" | "shield";
  title: string;
  paragraphsHtml: string[];
  closer?: string; // accent-colored last line in the card
}

export const commitments: Commitment[] = [
  {
    id: "persists-state",
    icon: "journal",
    title: "Persists state.",
    paragraphsHtml: [
      `State changes and effects are captured automatically, without serialization, state machines, or annotations. Agents suspend for days or weeks at zero compute and zero memory cost, resuming with the same memory, locals, and call stack.`,
    ],
    closer: "Treat memory as durable.",
  },
  {
    id: "executes-transactionally",
    icon: "exchange",
    title: "Executes transactionally.",
    paragraphsHtml: [
      `Agent logic, tools, and inter-agent calls run exactly once — not "at-least-once with idempotency disclaimers." Transient failures retry without exiting your agent; any interruption — restart, redeploy, eviction, hardware fault — recovers with full state.`,
    ],
    closer: "Ship code that runs exactly once.",
  },
  {
    id: "enforces-policy",
    icon: "shield",
    title: "Enforces every policy.",
    paragraphsHtml: [
      `Every agent and tool runs in its own WASM sandbox — millisecond startup, megabytes of memory — with capabilities that can't be forged or leaked<span class="marker">*</span>. Rate, capacity, and concurrency limits are runtime-enforced; every authorization is journaled.`,
    ],
    closer: "Turn policies into guarantees.",
  },
];

// =============================================================================
// SECTION 4.5 — Framework vs runtime comparison
// =============================================================================

export interface ComparisonRow {
  icon: "cells" | "stack" | "cycle" | "bounded" | "brackets";
  framework: string;
  golem: string;
  why: string;
}

export const frameworkVsRuntime = {
  eyebrow: "Why not LangChain?",
  heading: "LangChain leaves you the hard parts.",
  leadHtml: `Tool calls firing twice. State lost mid-node. SQL checkpointers under load. These aren't problems LangChain solves — they're runtime problems. Golem solves them as runtime guarantees.`,
  columns: {
    framework: "AI Framework",
    golem: "Golem Runtime",
    why: "Why it matters",
  },
  rows: [
    {
      icon: "cells",
      framework:
        "Agents share host resources; tenant isolation depends on developer-enforced discipline",
      golem: "Each agent owns its own filesystem, SQLite database, and environment",
      why: "Cross-tenant leaks become structurally impossible",
    },
    {
      icon: "stack",
      framework:
        "Durability is opt-in and coarse — recovery restarts from last boundary, losing in-flight state",
      golem:
        "Every state change captured automatically; in-flight state survives any failure or suspension",
      why: "No state is ever lost to failure or suspension",
    },
    {
      icon: "cycle",
      framework:
        "Auto-retry only works for idempotent tools; the rest require developer-managed safety logic",
      golem:
        "Agent logic and tools — internal or external — execute durably with exactly-once semantics",
      why: "Infrastructure failures never cause partial or duplicate work",
    },
    {
      icon: "bounded",
      framework:
        "Authority enforced by developer-written code and LLM prompts; both fail when their authors do",
      golem: "Capabilities bounded by the runtime — code can only do what it's granted",
      why: "Buggy or malicious code can't exceed what it was granted",
    },
    {
      icon: "brackets",
      framework:
        "Waits, retries, and HITL require explicit state-machine code at framework boundaries",
      golem: "Any flow is just code — suspension, retries, and resumption are runtime behaviors",
      why: "No state-machine code to write or maintain",
    },
  ] as ComparisonRow[],
  closer: "Runtimes deliver what frameworks can't even promise.",
};

// =============================================================================
// SECTION 4.7 — Substrate metric bar (between FvR and What You Build)
// =============================================================================

export const metricBar = {
  metrics: [
    { value: "10,000+", label: "Active agents per node" },
    { value: "2 ms", label: "Agent cold start" },
    { value: "1 MB", label: "Min sandbox memory" },
    { value: "0 CPU/RAM", label: "Idle resource cost" },
  ],
};

// =============================================================================
// SECTION 5 — Bring your stack
// =============================================================================

export const bringStack = {
  eyebrow: "Bring your stack",
  heading: "Your libraries. Our runtime.",
  paragraphsHtml: [
    `Bring your favorite LLM SDKs, your tool libraries, your utilities — anything that's just code. They run on Golem, and your agent logic and tool primitives inherit the runtime's guarantees, without modification.`,
    `Use Golem's lightweight SDKs only when you want runtime-specific features: durability hooks, forking, rollbacks, agent and tool discovery. Frameworks that bring their own runtime aren't officially supported today.`,
  ],
};

export const frameworks: string[] = [
  "LangChain.js",
  "Vercel AI SDK",
  "TanStack AI",
  "Effect AI",
  "Many other libraries & frameworks",
];

export const frameworksLabel = "Tested to work with";
export const frameworksNote = "Subject to WASM compatibility per language.";

// =============================================================================
// SECTION 7 — Open source. Your cloud. Your language.
// =============================================================================

export const openSource = {
  eyebrow: "Source-available. Your cloud. Your language.",
  heading: "Run it where you want. Write it how you like.",
  blocks: [
    {
      title: "BUSL-1.1 → Apache-2.0",
      body: "The runtime source is auditable, the WASM components are inspectable, and the license transitions to Apache-2.0 — staying out of your way today, fully permissive tomorrow.",
    },
    {
      title: "Your cloud.",
      body: "Run Golem where you run everything else — on a laptop, in Docker, in Kubernetes, on any cloud, or on-prem.",
    },
    {
      title: "Your language.",
      body: "Same runtime, same capabilities, same guarantees, same operational behavior across supported languages. ",
    },
  ],
  pedigreeHtml: `Built by the wizards behind <a href="https://zio.dev" target="_blank" rel="noopener"><strong>ZIO</strong></a> — the open-source effect system running in production at companies across fintech, ad tech, and AI infrastructure for the better part of a decade.`,
};

export const languages = [
  { name: "TypeScript", note: "Strongest surface" },
  { name: "Rust", note: "Substrate-credible" },
  { name: "Scala", note: "Effects-friendly" },
  { name: "MoonBit", note: "Small WASM" },
];

export const deployments = [
  { icon: "▸", label: "Laptop", note: "Local dev — byte-identical to prod" },
  { icon: "◇", label: "Docker", note: "Single binary, single config" },
  { icon: "⬢", label: "Kubernetes", note: "Helm chart, scale horizontally" },
  { icon: "☁", label: "Any cloud", note: "AWS, GCP, Azure, on-prem" },
  { icon: "★", label: "Golem Cloud", note: "Managed — when you want it" },
];

// =============================================================================
// SECTION 8 — Table-stakes strip
// =============================================================================

export interface TableStakeItem {
  icon: string;
  label: string;
  note: string;
  future?: boolean; // true ⇒ render with 1.6 marker asterisk
}

export const tableStakes = {
  eyebrow: "The full package",
  heading: "Bundled into the runtime.",
  items: [
    {
      icon: "▭",
      label: "OpenTelemetry built-in",
      note: "Every step traced, every metric auto-emitted",
    },
    {
      icon: "⌘",
      label: "MCP server, automatic",
      note: "Your agents are MCP servers out of the box",
    },
    { icon: "⇆", label: "Model-agnostic", note: "Any model via HTTP — routing stays in your code" },
    { icon: "◷", label: "Scheduled execution", note: "Cron-native, with durability across runs" },
    {
      icon: "⇶",
      label: "Webhook primitives",
      note: "Incoming events like HITL become awaitable promises",
    },
    { icon: "⇄", label: "Tool-calling protocols", note: "MCP, HTTP, RPC — all exactly-once" },
    {
      icon: "⌬",
      label: "Sandboxed by construction",
      note: "Every component runs in a WASM sandbox at instance cost",
    },
    {
      icon: "∿",
      label: "Streaming, durably",
      note: "WebSocket and SSE flows resume across deploys",
    },
    {
      icon: "⊟",
      label: "First-class quotas",
      note: "Rate, capacity, concurrency, GPU — one mechanism",
    },
    {
      icon: "⊜",
      label: "Replay-driven evaluation",
      note: "The oplog is your eval substrate, no separate harness",
    },
    {
      icon: "⤧",
      label: "A2A protocol interop",
      note: "Peer agents across runtime boundaries",
      future: true,
    },
    {
      icon: "⛨",
      label: "Tool middleware",
      note: "Polices and guardrails with irconglad guarantees",
      future: true,
    },
    {
      icon: "⚿",
      label: "First-class secrets",
      note: "Opaque handles, capability-gated reveal",
      future: true,
    },
    {
      icon: "⚷",
      label: "Per-tool capabilities",
      note: "Each tool call carries its own bounded authority",
      future: true,
    },
  ] as TableStakeItem[],
};

// =============================================================================
// SECTION 9 — Customer stories (GATED at launch)
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
  eyebrow: "In their own words",
  heading: "From teams shipping on Golem.",
  showAtLaunch: true,
  testimonials: [
    {
      quote:
        "Durable agents keep their state across crashes and restarts automatically. I write clean code instead of custom persistence and back-off logic.",
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
        "Like AWS Lambda, but the function has memory across invocations and durable reminders out of the box. Durability is invisible — exactly how infrastructure should feel.",
      name: "Seeta Ramayya Vadali",
      role: "Senior Software Consultant",
      company: "Building Seeta AI Assistant",
      avatar: {
        filename: "seeta-ramayya-vadali.jpg",
        alt: "Seeta Ramayya Vadali",
      },
      productLogo: { filename: "seeta-ai-assistant.png", alt: "Seeta AI Assistant" },
    },
  ] as Testimonial[],
};

// =============================================================================
// SECTION 10 — Quickstart
// =============================================================================

export const quickstart = {
  // headingLines render with <br> between.
  headingLines: ["Crash your first agent in five minutes.", "Watch it come back."],
  lead: "Scaffold a durable agent, run it locally, kill the process at any line, and watch it resume exactly where it stopped.",
  installCommand: `# Install: download from github.com/golemcloud/golem/releases
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
