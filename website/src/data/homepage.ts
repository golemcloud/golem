// Homepage copy — single source of truth for all visitor-facing prose on /
//
// Editing notes:
//   - Plain strings are rendered as-is.
//   - HTML strings (paragraphs in cards, hero expander, etc.) are rendered via
//     `set:html` in the components. Use <strong>, <em>, and <code>.

// =============================================================================
// SECTION 1 — Hero
// =============================================================================

export const hero = {
  // Small release line above the headline. Update with each major release.
  eyebrow: {
    label: "Golem 1.6 · Agents, Tools, and Artifacts",
    href: "/blog/golem-1-6-agents-tools-and-artifacts",
  },
  // headingLines render as <br>-separated lines inside the same <h1>.
  headingLines: ["Agents that never fail.", "Policies that never bend."],
  // expander is HTML (uses <strong>, <br>).
  expanderHtml: `Deploy <strong>agents, their tools, and their artifacts</strong> to one durable runtime. State persists automatically, every tool call executes exactly once, and the host enforces every policy — whoever, or whatever, wrote the code.`,
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
  lead: "Typed agents in TypeScript, Effect, Rust, Go, Scala, or MoonBit. State survives crashes and redeploys. Tool calls never fire twice. Your code — and the host — decide what's allowed.",
  defaultLang: "typescript" as const,
  tabs: [
    { id: "typescript", label: "TypeScript", filename: "orders-agent.ts", lang: "typescript" },
    { id: "effect", label: "Effect", filename: "orders-agent.ts", lang: "typescript" },
    { id: "rust", label: "Rust", filename: "orders_agent.rs", lang: "rust" },
    { id: "go", label: "Go", filename: "orders.go", lang: "go" },
    { id: "scala", label: "Scala", filename: "OrdersAgent.scala", lang: "scala" },
    { id: "moonbit", label: "MoonBit", filename: "orders_agent.mbt", lang: "moonbit" },
  ],
  snippets: {
    typescript: `export const Orders = defineAgent({
  name: 'Orders',
  id: { customerId: z.string() },
  http: http.mount('/orders/{customerId}'),
  config: { systemPrompt: z.string() },
  methods: {
    handle: method({
      input: { request: z.string(), orderId: z.string() },
      returns: z.object({ resolved: z.boolean() }),
    }),
  },
})

export const OrdersImpl = Orders.implement({
  init: () => ({ history: [] as Message[] }),
  methods: {
    async handle({ request, orderId }) {
      // No DB writes — this push survives crashes, deploys, host migrations
      this.history.push({ role: 'user', content: request })

      // LLM sees full conversation; system prompt comes from typed config
      const outcome = await llm.run({
        prompt: this.config.systemPrompt, history: this.history,
        tools: [cancelOrder, changeAddress], context: { orderId },
      })
      this.history.push({ role: 'assistant', content: outcome.message })

      // Refunds aren't in the LLM's toolset — agent code gates them via HITL
      if (outcome.needsRefund) {
        // This await can sit for days at zero cost — no queue, no cron, no state table
        const webhook = createWebhook()
        await notifyApprover(webhook.getUrl(), outcome)
        const { approved } = (await webhook).json()
        if (approved) {
          // Crash, retry, restart — still one charge. No silent double-charges.
          const result = await refundOrder({ orderId, amount: outcome.refundAmount })
          this.history.push({ role: 'tool', content: JSON.stringify(result) })
        }
      }
      return { resolved: true }
    },
  },
})`,

    effect: `class OrdersConfig extends defineConfig("Orders.Config", {
  systemPrompt: Schema.String,
}) {}

export const Orders = defineAgent({
  name: "Orders",
  id: { customerId: Schema.String },
  http: Http.mount("/orders/{customerId}"),
  config: OrdersConfig,
  methods: {
    handle: method({
      input: { request: Schema.String, orderId: Schema.String },
      success: Schema.Struct({ resolved: Schema.Boolean }),
    }),
  },
}).implement<Ref.Ref<Message[]>>({
  init: () => Ref.make<Message[]>([]),
  methods: (history) => ({
    handle: ({ request, orderId }) =>
      Effect.gen(function* () {
        // No DB writes — this update survives crashes, deploys, host migrations
        yield* Ref.update(history, (h) => [...h, { role: "user", content: request }])

        // LLM sees full conversation; system prompt comes from typed config
        const config = yield* OrdersConfig
        const outcome = yield* llm.run({
          prompt: yield* config.systemPrompt, history: yield* Ref.get(history),
          tools: [cancelOrder, changeAddress], context: { orderId },
        })
        yield* Ref.update(history, (h) => [...h, { role: "assistant", content: outcome.message }])

        // Refunds aren't in the LLM's toolset — agent code gates them via HITL
        if (outcome.needsRefund) {
          // This await can sit for days at zero cost — no queue, no cron, no state table
          const webhook = yield* Webhook.create
          yield* notifyApprover(webhook.url, outcome)
          const { approved } = yield* (yield* webhook.await).decode(Approval)
          if (approved) {
            // Crash, retry, restart — still one charge. No silent double-charges.
            const result = yield* refundOrder({ orderId, amount: outcome.refundAmount })
            yield* Ref.update(history, (h) => [...h, { role: "tool", content: JSON.stringify(result) }])
          }
        }
        return { resolved: true }
      }),
  }),
})`,

    rust: `#[derive(ConfigSchema)]
pub struct OrdersConfig { pub system_prompt: String }

#[agent_definition(mount = "/orders/{customer_id}")]
pub trait Orders {
    fn new(customer_id: String, #[agent_config] config: Config<OrdersConfig>) -> Self;
    async fn handle(&mut self, request: String, order_id: String) -> bool;
}

struct OrdersImpl { config: Config<OrdersConfig>, history: Vec<Message> }

#[agent_implementation]
impl Orders for OrdersImpl {
    fn new(_customer_id: String, #[agent_config] config: Config<OrdersConfig>) -> Self {
        Self { config, history: Vec::new() }
    }

    async fn handle(&mut self, request: String, order_id: String) -> bool {
        // No DB writes — this push survives crashes, deploys, host migrations
        self.history.push(Message::user(request));

        // LLM sees full conversation; system prompt comes from typed config
        let system_prompt = self.config.get().expect("config").system_prompt;
        let outcome = llm::run(
            &system_prompt, &self.history,
            vec![cancel_order(), change_address()], order_id.clone(),
        ).await;
        self.history.push(Message::assistant(outcome.message.clone()));

        // Refunds aren't in the LLM's toolset — agent code gates them via HITL
        if outcome.needs_refund {
            // This await can sit for days at zero cost — no queue, no cron, no state table
            let webhook = create_webhook().expect("webhook");
            notify_approver(webhook.url(), &outcome).await;
            let approval: Approval = webhook.await.json().expect("approval");
            if approval.approved {
                // Crash, retry, restart — still one charge. No silent double-charges.
                let result = refund_order(order_id, outcome.refund_amount).await;
                self.history.push(Message::tool(result.to_string()));
            }
        }
        true
    }
}`,

    go: `type ID struct{ Customer string }
type Config struct{ SystemPrompt string }
type HandleIn struct{ Request, OrderID string }

var Orders = golem.DefineConfiguredAgent[ID, Config](golem.Spec{
	Name: "Orders",
	HTTP: &golem.Mount{Path: "/orders/{customer}"},
})

var Handle = Orders.Method[HandleIn, bool]("handle")

type state struct{ history []Message }

func (s *state) add(role, content string) {
	s.history = append(s.history, Message{role, content})
}

var agent = Orders.Implement(func(ID) *state { return &state{} })

func init() {
	agent.Handle(Handle, func(ctx *golem.Context[state], in HandleIn) bool {
		// No DB writes — this append survives crashes, deploys, host migrations
		ctx.State.add("user", in.Request)

		// LLM sees full conversation; system prompt comes from typed config
		outcome := llm.Run(ctx.Config(Orders).SystemPrompt, ctx.State.history,
			[]Tool{cancelOrder, changeAddress}, in.OrderID)
		ctx.State.add("assistant", outcome.Message)

		// Refunds aren't in the LLM's toolset — agent code gates them via HITL
		if outcome.NeedsRefund {
			// This await can sit for days at zero cost — no queue, no cron, no state table
			webhook := golem.NewWebhook[Approval]()
			notifyApprover(webhook.URL(), outcome)
			if webhook.MustAwait().Approved {
				// Crash, retry, restart — still one charge. No silent double-charges.
				result := refundOrder(in.OrderID, outcome.RefundAmount)
				ctx.State.add("tool", result.String())
			}
		}
		return true
	})
}`,

    scala: `final case class OrdersConfig(systemPrompt: String) derives Schema

@agentDefinition(mount = "/orders/{customerId}")
trait Orders extends BaseAgent with AgentConfig[OrdersConfig]:
  class Id(val customerId: String)
  def handle(request: String, orderId: String): Future[Boolean]

@agentImplementation()
final class OrdersImpl(customerId: String, config: Config[OrdersConfig]) extends Orders:
  private var history: Vector[Message] = Vector.empty

  override def handle(request: String, orderId: String): Future[Boolean] =
    // No DB writes — this append survives crashes, deploys, host migrations
    history = history :+ Message("user", request)

    for
      // LLM sees full conversation; system prompt comes from typed config
      outcome <- llm.run(
        prompt = config.value.systemPrompt, history = history,
        tools = Seq(cancelOrder, changeAddress), context = Map("orderId" -> orderId))
      _ = history = history :+ Message("assistant", outcome.message)

      // Refunds aren't in the LLM's toolset — agent code gates them via HITL.
      // This await can sit for days at zero cost — no queue, no cron, no state table
      approved <-
        if outcome.needsRefund then awaitApproval(HostApi.createWebhook(), outcome)
        else Future.successful(false)

      // Crash, retry, restart — still one charge. No silent double-charges.
      _ <-
        if approved then refundOrder(orderId, outcome.refundAmount)
          .map(result => history = history :+ Message("tool", result.toString))
        else Future.unit
    yield true`,

    moonbit: `#derive.config
pub(all) struct OrdersConfig { system_prompt : String }

#derive.agent
#derive.mount("/orders/{customer_id}")
struct Orders {
  config : @config.Config[OrdersConfig]
  mut history : Array[Message]
}

fn Orders::new(customer_id : String, config : @config.Config[OrdersConfig]) -> Orders {
  let _ = customer_id
  { config, history: [] }
}

pub async fn Orders::handle(self : Self, request : String, order_id : String) -> Bool {
  // No DB writes — this push survives crashes, deploys, host migrations
  self.history.push({ role: "user", content: request })

  // LLM sees full conversation; system prompt comes from typed config
  let outcome = @llm.run(
    prompt = self.config.value.system_prompt, history = self.history,
    tools = [cancel_order(), change_address()], context = { "order_id": order_id },
  )
  self.history.push({ role: "assistant", content: outcome.message })

  // Refunds aren't in the LLM's toolset — agent code gates them via HITL
  if outcome.needs_refund {
    // This await can sit for days at zero cost — no queue, no cron, no state table
    let webhook = @webhook.create()
    notify_approver(webhook.url(), outcome)
    if webhook.wait().text() == "approved" {
      // Crash, retry, restart — still one charge. No silent double-charges.
      let result = refund_order(order_id, outcome.refund_amount)
      self.history.push({ role: "tool", content: result.to_json().stringify() })
    }
  }
  true
}`,
  } as Record<string, string>,
};

// =============================================================================
// SECTION 2.5 — One runtime: agents, tools, artifacts
// =============================================================================

export interface TriadPillar {
  id: "agents" | "tools" | "artifacts";
  title: string;
  bodyHtml: string;
}

export const triad = {
  eyebrow: "One runtime",
  heading: "Deploy agents, their tools, and their artifacts.",
  pillars: [
    {
      id: "agents",
      title: "Agents",
      bodyHtml: `Typed, durable agents in six SDKs. Streaming methods, read-only methods, reflection, and state that survives anything.`,
    },
    {
      id: "tools",
      title: "Tools",
      bodyHtml: `Typed tools with host-enforced middleware, a built-in toolkit — shell, files, git, Node, npm, TypeScript, web fetch — and MCP in both directions.`,
    },
    {
      id: "artifacts",
      title: "Artifacts",
      bodyHtml: `The apps, pages, and files agents ship to people — served by Golem, backed by the agent, with login and resumable streams built in.`,
    },
  ] as TriadPillar[],
  closerHtml: `The harness chooses what an agent does. <strong>The runtime decides what it can do.</strong>`,
};

// =============================================================================
// SECTION 2.7 — Business coding agents
// =============================================================================

export const codingAgent = {
  eyebrow: "Business coding agents",
  heading: "A coding agent, entirely in Golem.",
  paragraphsHtml: [
    `The coding agents that matter most are the ones business users never see: the support bot that writes a script to reconcile an account, the assistant that builds the report from three systems.`,
    `On Golem, the shell, files, git, Node, npm, and TypeScript run inside the sandbox as isolated tool calls — under a path policy the agent can't bypass and a permission card that names the only hosts it may reach. No Linux VM to boot, sync, or pay for. Kill the server mid-task and the agent picks up where it stopped.`,
  ],
  toolkitLabel: "Built-in toolkit",
  toolkit: [
    "bash",
    "read-file",
    "write-file",
    "edit-file",
    "ls",
    "grep",
    "git",
    "node",
    "npm",
    "npx",
    "tsc",
    "web-fetch",
    "path-policy",
  ],
  terminal: {
    label: "golem ssh",
    lines: [
      { kind: "cmd", text: `golem ssh 'ReportAgent("q3")'` },
      { kind: "prompt", text: "npm install csv-parse" },
      { kind: "out", text: "added 1 package in 2s" },
      { kind: "prompt", text: "node build-report.js && ls reports" },
      { kind: "out", text: "q3-summary.html" },
    ],
  },
  noteHtml: `Harness inside or outside the sandbox — your choice. The guarantees come from the host, not from where the loop runs.`,
  cta: { label: "Read the 1.6 announcement →", href: "/blog/golem-1-6-agents-tools-and-artifacts" },
};

// =============================================================================
// SECTION 4.2 — Artifacts
// =============================================================================

export const artifacts = {
  eyebrow: "Artifacts",
  heading: "Agents ship things people use.",
  leadHtml: `Front-ends, reports, live output — served by the same runtime that runs the agent, from the same deployment, under the same authority, with the same audit trail.`,
  points: [
    { title: "HTTP routers", body: "Mount any Fetch-compatible framework, and publish OpenAPI." },
    {
      title: "Static assets",
      body: "Versioned with the deployment, served without waking an agent.",
    },
    { title: "Live files", body: "Agents publish what they write — reports, builds, workspaces." },
    {
      title: "Front-end login",
      body: "OAuth2 with PKCE for single-page apps, tokens issued by Golem.",
    },
    {
      title: "Durable Streams",
      body: "Stream to the browser; a reload picks up where it left off.",
    },
    {
      title: "Subdomains",
      body: "A real URL from the first golem deploy, locally and in the cloud.",
    },
  ],
  filename: "router.ts",
  snippet: `export const AppRouter =
  defineHttpRouter('AppRouter')
    .mount('/app')
    .implement((req) => app.fetch(req))`,
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
    { kind: "image", filename: "golem-journai.png", alt: "JournAI" },
    { kind: "image", filename: "johnethel-lms.png", alt: "Johnethel LMS" },
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
    title: "Automatically persists state.",
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
      `Code keeps running exactly once through any interruption, as if nothing happened. A tool call that crashed mid-execution completes; a workflow paused for days picks up where it stopped. This is transactional code execution.`,
    ],
    closer: "Ship code that runs exactly once.",
  },
  {
    id: "enforces-policy",
    icon: "shield",
    title: "Enforces every policy.",
    paragraphsHtml: [
      `Every agent and every tool call runs in its own WebAssembly instance — its own memory, no system calls. Authority comes from <strong>permission cards</strong> the runtime mints: they narrow, never widen, and revocation cascades. Tool middleware runs in the host on every call, secrets are opaque handles, and every decision is journaled.`,
    ],
    closer: "Turn policies into guarantees.",
  },
];

// =============================================================================
// SECTION 4.5 — Framework vs runtime comparison
// =============================================================================

export interface ComparisonRow {
  icon: "cells" | "stack" | "cycle" | "bounded" | "gate" | "brackets";
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
      golem: "Each agent owns its own memory, filesystem, and environment",
      why: "Cross-tenant leaks become structurally impossible",
    },
    {
      icon: "stack",
      framework:
        "Durability is opt-in and coarse — recovery restarts from last boundary, losing in-flight state",
      golem:
        "No checkpoints, steps, or annotations — the whole agent resumes mid-execution, after any failure or suspension",
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
      golem:
        "Authority carried by permission cards the runtime mints — code can only do what its cards and imports allow",
      why: "Buggy or malicious code can't exceed what it was granted",
    },
    {
      icon: "gate",
      framework: "Guardrails run in-process; generated code can route around them",
      golem: "Tool middleware enforced by the host on every call",
      why: "Policies hold even for code the model wrote",
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
    `Use Golem's lightweight SDKs only when you want runtime-specific features: durability hooks, forking, rollbacks, agent and tool discovery. MCP servers you import inherit the same durability and middleware. Frameworks that bring their own runtime aren't officially supported today.`,
  ],
};

export const frameworks: string[] = [
  "OpenAI & Anthropic SDKs",
  "Vercel AI SDK",
  "TanStack AI",
  "Effect AI",
  "Zod, Valibot, ArkType",
  "Many other libraries & frameworks",
];

export const frameworksLabel = "Tested to work with";
export const frameworksNote = "Subject to WASM compatibility per language.";

// =============================================================================
// SECTION 7 — Open source. Your cloud. Your language.
// =============================================================================

export const openSource = {
  eyebrow: "Open source. Your cloud. Your language.",
  heading: "Run it where you want. Write it how you like.",
  blocks: [
    {
      title: "BUSL‑1.1 → Apache‑2.0",
      body: "The runtime source is auditable, the WASM components are inspectable, and the license transitions to Apache‑2.0 — staying out of your way today, fully permissive tomorrow.",
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
  { name: "Effect", note: "Effect-native TypeScript" },
  { name: "Rust", note: "Substrate-credible" },
  { name: "Go", note: "Idiomatic, generics-first" },
  { name: "Scala", note: "Effects-friendly" },
  { name: "MoonBit", note: "Small WASM" },
];

export const deployments = [
  { icon: "▸", label: "Laptop", note: "golem server run — the whole platform in one process" },
  { icon: "◇", label: "Docker", note: "Compose stacks, Postgres-backed" },
  { icon: "⬢", label: "Kubernetes", note: "Published service images, etcd-backed HA" },
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
}

export const tableStakes = {
  eyebrow: "The full package",
  heading: "Bundled into the runtime.",
  items: [
    {
      icon: "⌘",
      label: "MCP, both directions",
      note: "Export agents and tools; import any MCP server durably",
    },
    {
      icon: "⌥",
      label: "Built-in toolkit",
      note: "Shell, files, git, Node, npm, TypeScript, web fetch — in the sandbox",
    },
    {
      icon: "⛨",
      label: "Tool middleware",
      note: "Policies and guardrails, enforced by the host on every call",
    },
    {
      icon: "⚷",
      label: "Permission cards",
      note: "Runtime-minted authority that narrows, never widens",
    },
    {
      icon: "⚿",
      label: "First-class secrets",
      note: "Opaque handles; reveal only where granted, always journaled",
    },
    {
      icon: "◰",
      label: "Artifact hosting",
      note: "HTTP routers, static and live files, PKCE login",
    },
    {
      icon: "∿",
      label: "Durable Streams",
      note: "Resumable streams to any client, protocol-compatible URLs",
    },
    {
      icon: "◎",
      label: "Read-only methods",
      note: "Enforced at runtime, cached at the HTTP edge",
    },
    { icon: "⇄", label: "Tool-calling protocols", note: "MCP, HTTP, RPC — all exactly-once" },
    {
      icon: "⇶",
      label: "Webhook primitives",
      note: "Incoming events like HITL become awaitable promises",
    },
    {
      icon: "◷",
      label: "Scheduled execution",
      note: "Durable timers and scheduled invocations at zero idle cost",
    },
    {
      icon: "⊟",
      label: "User-defined quotas",
      note: "Rate, capacity, concurrency — throttle, reject, or terminate",
    },
    {
      icon: "▭",
      label: "OpenTelemetry built-in",
      note: "Every step traced, every metric auto-emitted",
    },
    {
      icon: "⊜",
      label: "Complete audit log",
      note: "Every action and authorization decision, recorded and replayable",
    },
    {
      icon: "⌬",
      label: "Sandboxed by construction",
      note: "Every agent and tool call runs in its own WebAssembly instance",
    },
    { icon: "⇆", label: "Model-agnostic", note: "Any model via HTTP — routing stays in your code" },
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
    {
      quote:
        "Tried Temporal, Dapr, Restate, and Fermyon Spin. Golem gives me what I'd given up on finding: code-level customizability with no-code-level operational simplicity.",
      name: "Samuel Iyeh",
      role: "AI Software Engineer",
      company: "Building Johnethel LMS",
      avatar: { filename: "samuel-iyeh.jpg", alt: "Samuel Iyeh" },
      productLogo: { filename: "johnethel-lms.png", alt: "Johnethel LMS" },
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
