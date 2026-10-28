// Visitor-facing copy for /use-cases.
// Editorial rationale and claim mapping: research/use-cases-selection-2026-10-27/page-decisions.md.

export const meta = {
  title: "Use Cases — Build reliable agents and workflows | Golem",
  description:
    "Build coding agents, research assistants, customer support, and more into your product. Custom tools run durably, with exactly-once execution and enforced policies.",
};

export const hero = {
  eyebrow: "Use cases",
  heading: "Build agents for work that matters",
  lead: "Build software. Investigate questions. Help customers. Put agents to work inside your product.",
  navigationLabel: "What are you building?",
  exploreLabel: "Explore use case",
};

export const featuredCase = {
  id: "coding",
  navLabel: "Software development",
  example: "Build a coding assistant that creates apps, fixes bugs, and maintains codebases.",
  title: "Coding agents that build and maintain software",
  pain: "A coding agent has to do more than suggest a patch. It edits files, calls tools, checks results, and publishes changes. A failure inside a tool should not lose the running work or apply the same change twice.",
  exampleTitle: "Build the coding assistant your product needs",
  runtime:
    "Build, deploy, and operate custom agents, tools, and reliable APIs with Golem. After a fault or interruption, the whole VM resumes mid-function with its memory, call stack, and files. Agent and tool calls run exactly once, under policies your agents cannot bypass.",
  toolChoice:
    "Write your agents and their tools with any Golem SDK. Choose the models, frameworks, and integrations your application needs.",
  parts: [
    {
      title: "Your agent",
      body: "Creates an app, diagnoses a bug, or maintains a codebase. Let it investigate, try a fix, and respond to developer feedback. Code review, release automation, and incident repair are other jobs you can build on the same runtime.",
    },
    {
      title: "Your tools",
      body: "Write repository, editing, and publishing tools with a Golem SDK. Each call runs in an isolated, durable VM. Connect external runners for native builds and tests. Serve interfaces the agent creates from its durable files through HTTP endpoints you declare.",
    },
    {
      title: "Your policies",
      body: "Scope which repositories and services the agent can reach. Require approval before merging or publishing. Golem runs your policy middleware outside the agent's control; the agent cannot skip the check or the approval wait.",
    },
  ],
  recoveryLabel: "Interrupted inside a tool call?",
  recovery:
    "Golem restores the tool's whole VM and resumes mid-function, with its memory, call stack, and files. The same invocation completes. Effects already recorded by Golem are replayed, not performed again: a file edit, for example, is not applied twice.",
  integrationLabel: "Calling an external service",
  integration:
    "Completed HTTP requests are not sent again. An interrupted request is retried with the same idempotency key, which Golem adds automatically. APIs that honor that key apply the operation once, too; choose that contract for consequential actions such as publishing changes or issuing refunds.",
  links: [
    {
      label: "Write your own tools",
      href: "https://learn.golem.cloud/v1.6/develop/tool-authoring",
    },
    {
      label: "Serve what your agents build",
      href: "https://learn.golem.cloud/v1.6/concepts/artifacts",
    },
  ],
};

export const jobs = [
  {
    id: "research",
    navLabel: "Research & knowledge",
    title: "Research agents that follow the question through",
    pain: "An answer may take many searches, source comparisons, and follow-up questions. A timeout should not erase an investigation or its working files.",
    example:
      "Build a research assistant, a knowledge search feature, or an agent that monitors a topic for your users.",
    help: "Connect your own search and retrieval tools. Keep the whole investigation running across failures and long waits, including unfinished tool calls. Preserve source material in the agent's durable file system, and let your application require review before it publishes findings. You choose the sources and how answers are checked.",
    links: [
      { label: "Build long-lived agents", href: "https://learn.golem.cloud/v1.6/concepts/agents" },
    ],
  },
  {
    id: "analysis",
    navLabel: "Data analysis",
    title: "Analysis agents that turn data into usable results",
    pain: "An analysis spans data access, generated code, intermediate files, and revisions. Users need the result; your product needs to keep untrusted code within its granted access.",
    example:
      "Add a data analyst to your product: query business data, investigate a metric, or generate a report and an interactive dashboard.",
    help: "Write tools for your data sources and run analysis in isolated tool VMs. Golem enforces their permissions and restores interrupted execution with its files and working state. Serve reports and interfaces the agent creates on demand from durable files, through HTTP endpoints you declare in agent code.",
    links: [
      {
        label: "Serve agent-generated reports and interfaces",
        href: "https://learn.golem.cloud/v1.6/concepts/artifacts",
      },
    ],
  },
  {
    id: "support",
    navLabel: "Customer support",
    title: "Support agents that can resolve the issue",
    pain: "Looking up an answer is one thing. Changing an account, replacing an order, or issuing a refund makes the agent part of your product's reliability promise. A restart must not turn one approved action into two.",
    example:
      "Build a support agent with your own account, order, and refund tools, inside the product your customers already use.",
    help: "Keep the conversation, account investigation, and in-progress tool calls together. Require approval for sensitive actions and resume after escalation without losing the wait. Golem enforces the permissions and policies you configure. For refunds, choose a provider that honors Golem's idempotency key.",
    links: [
      {
        label: "Enforce tool policies and approvals",
        href: "https://learn.golem.cloud/v1.6/develop/tool-security",
      },
    ],
  },
  {
    id: "personal",
    navLabel: "Personal assistants",
    title: "Personal assistants that remember and follow through",
    pain: "Scheduling a meeting or arranging a trip crosses conversations, calendars, and confirmations. The assistant may wait hours for a reply; a restarted process must not lose the request or its pending approval.",
    example: "Build a scheduling, travel, or task assistant with an ongoing agent for each user.",
    help: "Suspend while waiting, then resume the whole agent when work arrives. Suspended agents use no CPU or memory. Add your own calendar, messaging, and booking tools, with approval before commitments. Your application defines user identity and authorization; Golem enforces the tool permissions you grant.",
    links: [
      {
        label: "Create stateful agents for your users",
        href: "https://learn.golem.cloud/v1.6/concepts/agents",
      },
    ],
  },
  {
    id: "content",
    navLabel: "Content & media",
    title: "Content agents that carry work through to publication",
    pain: "A publishable asset takes source material, drafts, revisions, and review. An interruption during a tool call should not lose those files or bypass a decision to wait for an editor.",
    example:
      "Build writing, media production, or publishing features into your editor, CMS, or creative product.",
    help: "Connect the generation and editing services you choose through your own tools. Keep drafts in the agent's durable file system and require approval before publication. Agents and tools recover mid-function; Golem keeps the configured policy in force while the work waits.",
    links: [
      {
        label: "Build tools around your creative workflow",
        href: "https://learn.golem.cloud/v1.6/develop/tool-authoring",
      },
    ],
  },
  {
    id: "operations",
    navLabel: "Business operations",
    title: "Operations agents that move work across systems",
    pain: "An invoice, onboarding request, or order can span documents, approvals, and several systems. Duplicate writes or a lost approval leave people reconciling the damage.",
    example:
      "Build document intake, account onboarding, or order fulfillment into your software, using tools for your customers' systems.",
    help: "Run the agent, its tools, and background handlers durably. Calls to Golem agents and tools execute exactly once; database transactions through Golem's SDK never apply twice. Require review before changing records or committing money. Your application maps duplicate incoming requests to the same operation and defines compensation across systems; external APIs must honor the idempotency key to apply interrupted requests once.",
    links: [
      {
        label: "See the execution guarantees",
        href: "https://learn.golem.cloud/v1.6/concepts/durable-execution",
      },
    ],
  },
];

export const tools = {
  title: "Your tools are part of the runtime",
  body: "Implement the operations your product needs with any Golem SDK: TypeScript, Effect, Rust, Go, Scala, or MoonBit. Your tools get the same isolated execution, durability, and runtime-enforced policies as the bundled tools. Agents and workflows can call them, and you can expose them to MCP clients or share tool releases across environments.",
  link: {
    label: "Author and deploy a tool",
    href: "https://learn.golem.cloud/v1.6/develop/tool-authoring",
  },
};

export const fit = {
  title: "When reliability is part of the job",
  items: [
    {
      title: "Actions that must take effect once",
      body: "Your agent changes production systems, updates records, or handles money. You need exactly-once agent and tool execution, including recovery inside interrupted calls, with the right guarantees at external service boundaries.",
    },
    {
      title: "Policies that must hold",
      body: "Generated decisions must stay within your permissions and approval rules. Golem enforces those rules in the runtime, including while work waits for a person. You choose the grants and policy; the agent cannot bypass enforcement.",
    },
    {
      title: "Work that outlives a process",
      body: "Your application coordinates tools, waits for events, or keeps an ongoing relationship with a user. The whole execution must survive faults and long waits. A short, read-only model response may need much less infrastructure.",
    },
  ],
};

export const experiment = {
  title: "Start with one tool that matters",
  intro:
    "Follow the Quickstart, then use the tool-authoring guide to implement one operation your agent needs.",
  paragraphs: [
    "Connect your own system, grant only the access the tool needs, and add an approval policy before a sensitive action. Build the agent around your product's behavior and your users' needs.",
    "To test recovery, use the CLI to simulate a failure inside a tool after it changes a durable file. Check that the same invocation completes without applying the change twice, and that an approval wait survives. For external APIs, test interrupted requests against the provider's idempotency contract.",
  ],
  actions: [
    { label: "Start building", href: "https://learn.golem.cloud/quickstart" },
    {
      label: "Write your first tool",
      href: "https://learn.golem.cloud/v1.6/develop/tool-authoring",
    },
  ],
};

export const accessibility = { skip: "Skip to content" };
