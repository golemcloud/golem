---
title: "Your Old Agent Architecture Is Dead… Meet Its Replacement"
date: "2026-07-27T12:00:00Z"
author: "John A. De Goes"
tags: ["Industry Articles", "AI Agent"]
slug: "your-old-agent-architecture-is-dead-meet-its-replacement"
draft: false
---

![From hope-driven agent loops to compiled, verified, durable agent architecture](/blog-images/your-old-agent-architecture-is-dead-meet-its-replacement.png)

Today's state-of-the-art AI agent is a loop.

The loop maintains a conversation history, delegates to a model for thinking, executes the tool calls the model requests, appends the results, and repeats until the model declares victory.

Simple. Dumb. Increasingly obsolete.

More precisely: every meaningful property of today's agents is implemented as hope. We hope the model remembers its instructions. We hope it follows our rules. We hope it doesn't skip item 371 of 1,000. We hope its mistakes can be undone.

Yet, hope is not an architecture.

In this post, I'll show you what is—and why the industry's own state-of-the-art systems are already converging on it, whether they realize it or not.

## Hope #1: The Agent Is the Loop

Ask anyone to draw an "agent" and you'll get the same picture: non-determinism on top, dumb deterministic tools on the bottom. A model reasons; tools obey. Two layers, cleanly separated.

This picture is not merely simplistic. It's a category error—like insisting that programs must contain exactly one function call, located at the top.

The real structure of capable agentic systems is arbitrary interleaving. Code can invoke inference at any point in its execution. Inference can emit code at any point in its reasoning. And this alternation nests to any depth.

Consider a research tool: deterministic scaffolding (defined sources, deterministic scraping, deterministic search) wraps an inference step (summarize; decide whether to continue), which triggers more deterministic retrieval, which feeds more inference.

Or consider QA for a game: an agent writes a deterministic test harness for a level, and inside that harness sits an inference call doing visual spot-checks—non-determinism embedded within determinism embedded within non-determinism.

The agent is not just the loop at the top. The agent is the whole recursive structure—code containing inference, which writes code, which contains inference, all the way down. The model is not the architecture; it is a component type, appearing at every depth, like function calls in a program.

This is not speculation about the future. It is a description of the present.

In May 2026, Anthropic shipped [dynamic workflows](https://claude.com/blog/introducing-dynamic-workflows-in-claude-code) in [Claude Code](https://www.claude.com/product/claude-code). When a workflow kicks off, Claude—non-deterministically—writes a deterministic JavaScript orchestration script, which fans work out across tens to hundreds of parallel subagents, whose results are adversarially checked by other agents that try to refute them, iterating until the answers converge.

Read that structure carefully: inference writes code, which invokes inference, which writes code, which is validated by inference, composed deterministically. That's four or five alternations of the sandwich—in a shipping product.

The showcase result: Jarred Sumner used dynamic workflows to [port Bun from Zig to Rust](https://bun.com/blog/bun-in-rust)—roughly 750,000 lines of [Rust](https://www.rust-lang.org/), 99.8% of the existing test suite passing, eleven days from first commit to merge. One workflow mapped the right Rust lifetime for every struct field in the [Zig](https://ziglang.org/) codebase. The next wrote every `.rs` file as a behavior-identical port of its `.zig` counterpart, hundreds of agents in parallel, two reviewers on each file. A fix loop then drove the build and test suite until both ran clean.

No loop-with-tools does this. No context window holds this. Anthropic didn't set out to validate a theory of agent architecture; they set out to port 750,000 lines of code, and this shape is what worked.

Now, dynamic workflows is a restricted form of the architecture I'm describing: the recursion is depth-capped (one privileged orchestration layer; ordinary tool-loop subagents beneath it), domain-locked to software engineering (where the test suite provides a free verification oracle), and guarded by product mechanisms (confirmation dialogs, admin settings) rather than by the architecture itself.

Those restrictions are the right call for a product shipping today. But they are restrictions, not essence. The unrestricted form—agents writing code that spawns agents that write code, to any depth, in any domain, with guardrails as first-class citizens of the structure—is where this goes.

## Hope #2: The Model Does the Bookkeeping

Why does the recursive form win? Because of where it puts the non-determinism.

Ask a model to transform 1,000 sentences from present tense to past tense, directly, in one pass. It will make mistakes. It will skip sentences. It will drift at some sentence or another.

Not because models can't conjugate verbs—but because attention over a long context is a terrible implementation of a for-loop. You are asking the model to simulate a control structure it could instead write.

Now let the model write the for-loop, with a scoped subagent call per sentence. Reliability skyrockets. Not because the model got smarter—because the model was removed from the part of the job it was bad at. Iteration is now code: exact, complete, un-skippable. Each inference call is scoped down to a task small enough that its error rate is low.

Add an independent auditor—another scoped inference call per sentence, checking the first one, wired together deterministically—and reliability compounds again. The residual error rate becomes the product of two independent small numbers instead of one large one.

This is precisely the structure dynamic workflows industrialized: adversarial reviewer agents, two reviewers per file, refutation until convergence. And notice what makes it possible: code holds the plan. An agent loop cannot orchestrate its own adversaries—it would have to remember to distrust itself, inside the same context window that is generating the errors.

In the loop model, verification is a virtue the model must remember. In the program model, verification is a structure the code cannot skip.

The general principle: decomposition, iteration, arithmetic, aggregation, sequencing, retry, and verification-plumbing are code's native strengths. Judgment under ambiguity is inference's native strength. A well-built agent is code doing everything code can do, invoking inference only at the joints where judgment is irreducible.

Every unit of work you leave inside a model's forward pass runs on an unreliable substrate. Every unit you relocate into code runs on a perfectly reliable one. The engineering discipline of agent-building is the discipline of shrinking the non-deterministic surface to the minimum that genuinely requires judgment.

None of this is an insult to models, by the way. Humans have the same limitations.

No human transforms 1,000 sentences by hand without skipping one—which is why we invented checklists, spreadsheets, and scripts. Externalizing bookkeeping into reliable structure is what intelligence does when it is being used well. The two-layer picture treats the model as a brain that must hold the entire task in its head. The recursive picture treats it the way we treat competent human intelligence: something you surround with tools, process, and verification, precisely because you respect both its strengths and its limits.

There is a bonus, too: scoped inference calls are smaller, cheaper, easier to parallelize, and easier to usefully cache. The same restructuring that buys reliability also buys performance and efficiency. One heroic long-context generation is the most expensive and least reliable way to spend your inference budget.

## Hope #3: Think One Line, Execute One Line

There is a second failure mode in today's agents, orthogonal to the first: when effects happen, relative to when thinking happens.

The dominant model is interactive: the agent thinks, executes one action, observes, thinks again, executes again. One line of code—or one tool call—at a time, each executed immediately.

For software development, this is often exactly right. A developer's coding agent mutates the state of a filesystem. Development is inherently iterative, and the consequences are nearly nonexistent: you are sculpting raw clay—editing files, running the compiler, fixing errors—and `git revert` is always one step away.

But interactive execution is only correct inside a specific quadrant, defined by two axes.

The first axis is reversibility. When the state is a filesystem, mistakes are free. When the state is live systems—the CRM, the payment processor, the email account—mistakes are forever. No shell command has ever unsent an email. No REPL has ever undone a refund.

The second axis is scale. When the task fits in one head—one context window, one attention span—interactive execution works. When the task is 750,000 lines across thousands of files, no loop survives it. The plan drifts, the context overflows, and file 400 gets treated differently than file 40, for no reason anyone can reconstruct.

Interactive execution is correct exactly where the task is small and the state is revertible. Everywhere else, it fails—and it fails for a reason that cannot be patched, even in theory:

If you plan and execute one line at a time, there is nothing to analyze. The "plan" never exists as an artifact; it only ever exists one already-executed line at a time. You cannot type-check it. You cannot test it. You cannot review it. You cannot simulate it. Every mistake ships, because there is no moment—not one—between the mistake being made and the mistake being real.

We should probably name this dominant paradigm what it is: Hope-Driven Execution. Behold the agent hoping its way, one irreversible tool call at a time, through a customer refund. Behold its sibling hoping its way through file 400 of a 2,000-file migration. Better not make mistakes!

The alternative is old, boring, and correct: the plan should be a program.

A program is a plan reified as a value in a file system. Values can be type-checked. Values can be run against mocks and test tenants. Values can be linted, guardrailed, simulated, and reviewed—by humans, or by adversarial agents—before a single real effect fires. And values that survive review become assets: reusable on the next similar task, modifiable at far lower risk than regeneration from scratch.

This idea is not new. Functional programmers have argued for decades that effects should be first-class values rather than side-effecting statements—plans you can inspect, compose, and test before running them. I spent a good part of my career [building exactly that](https://zio.dev/). And I will be honest about how it went: for human developers, the economics never fully closed. Hand-writing the reification (and associated monadic plumbing) was a ceremony most teams wouldn't pay for, and the safety it bought rarely justified the discipline it cost.

But the calculus has flipped, and it is worth examining why. The cost side collapsed: agents write the reified plan for free—no ceremony, no discipline tax, no convincing your team. And the benefit side exploded: when the executor is an autonomous system operating at machine speed against irreversible state, "analyzable before execution" stops being a nice-to-have and becomes the entire ballgame. A discipline too expensive for humans arrived at zero cost, precisely when it became mandatory.

The industry is converging here too, from a different direction. In late 2025, [Cloudflare](https://blog.cloudflare.com/code-mode/) and [Anthropic](https://www.anthropic.com/engineering/code-execution-with-mcp) independently discovered that agents handle tools far better when the tools are presented as a typed API the model writes code against, rather than as individual tool calls. Cloudflare's stated reason is telling: models have seen an enormous amount of real-world code in training, but only a small set of contrived tool-call examples. And the efficiency gains are not subtle—Anthropic reported token reductions on the order of 98% for multi-tool workflows, and Cloudflare later [exposed their entire API surface](https://blog.cloudflare.com/code-mode-mcp/), thousands of endpoints, through two tools consuming about a thousand tokens: `search()` and `execute()`.

When two competitors independently abandon the tool-call interface for the code interface, that is not fashion. That is convergent evolution under selection pressure.

## Hope #4: The Rules Are in AGENTS.md

Every team running agents today maintains a rules file. [AGENTS.md](https://agents.md/), CLAUDE.md, a system prompt—the name varies; the mechanism doesn't. English sentences, placed in context, describing what the agent must and must not do.

Everyone who maintains one of these files has watched an agent ignore it.

This is not a bug to be fixed by better prompting. It is the mechanism working as designed—because these are not rules. Nothing forces the agent to follow them. They are requests, enforced by attention, and attention is a depreciating asset: the rule at position 200 of the context competes for salience with the build log at position 80,000. As the context grows, your "rules" dissolve into the noise, probabilistically, silently.

Calling them rules is a category error: the name promises a guarantee the mechanism cannot deliver. A comment in your codebase saying "this function must never be called with a null argument" is not a null check. A wiki page saying "always review before merging" is not branch protection. Prose adjacent to a system is not a property of the system.

The industry's fix is bigger models and longer context—buying rule-compliance with inference cost, forever, at runtime, on every single step. Think about what that means: you are paying a model to remember your rules. It is the most expensive implementation of an if-statement ever devised.

There is a better way, and it is staring us in the face.

AGENTS.md is source code. It is a program, written in English, that today runs on the world's worst interpreter—the model's attention, re-executed probabilistically at every step. The alternative is to compile it: spend inference once, at setup time, to translate English rules into deterministic middleware installed in the agent's harness.

"Don't commit unless tests pass" compiles to tool middleware that intercepts `git commit` and refuses unless the test suite has run and is green. "Never remove tests" compiles to middleware on the file-editing tool that rejects any diff deleting or disabling existing tests.

The rule stops being a sentence the model might weigh. It becomes a structural property of the world the agent inhabits. Attention decays; middleware doesn't. And middleware cannot be prompt-injected, because middleware isn't listening. A malicious instruction smuggled into the agent's context can argue with a rule that lives in that context. It cannot argue with an interceptor that doesn't parse natural language at all.

"But most of my rules are fuzzy," you object. "You can't compile 'Write idiomatic code' into middleware."

Can't you? Notice, first, that "write idiomatic code" doesn't work on humans either. It means nothing to a junior developer, and it means nothing to an agent, and for the same reason: it is not a rule. It is a compressed pointer to a body of knowledge that nobody bothered to expand. Agents receiving it still write slop—as do juniors.

So expand it. A bootstrap agent that takes the time to research what "idiomatic" means for this language, this stack, and this codebase can decompose the sentence into things that actually check: cyclomatic complexity ceilings, method-size limits, naming conventions, code organization, import discipline. Fuzzy rule in; deterministic checks out. The fuzziness was never essential—it was laziness, tolerable when the reader was human and catastrophic now that it isn't.

And whatever survives decomposition—the truly irreducible residue that no amount of research can translate into a check?

What you cannot enforce deterministically is wishful thinking. Maybe it buys you something sometimes. You cannot rely on it. The architecture's refusal to compile your rule is not a limitation of the architecture—it is a bug report against your expectations.

We know this is true, because humanity already ran this migration once. "Write idiomatic code" sat in style guides for decades, meaning nothing, until we compiled it—into formatters, linters, complexity budgets, CI gates, branch protection. Every rule your team actually relies on is one that got translated from English into an enforcement mechanism. Everything still in the wiki is, and always was, decoration. Agent architecture is not inventing a new discipline. It is recapitulating the entire history of software engineering process, at a thousand times the speed—and rules files are currently stuck at the wiki stage.

[Hooks](https://code.claude.com/docs/en/hooks)—as found in Claude Code and elsewhere—are the prior art here, and they exist for very good reason: practitioners already discovered that prompts don't enforce. But hooks today are hand-written, by the minority of developers who bother, one static configuration per project. They are the hand-written assembly of this architecture: proof that the target layer works, awaiting the compiler that makes it universal, automatic, and—as we're about to see—per-task.

## The Architecture

Assemble the pieces and a picture emerges. Let me describe it as the lifecycle of a single task.

A task arrives. It is not handed to the inner agent. It is handed to a bootstrap agent, whose job is not to attempt the task but to construct the machine that will attempt the task safely.

The bootstrap agent reads the rules file. It reads the task. It researches what it doesn't know—the stack, the codebase, the systems involved. Then it compiles a custom harness for this specific task: the minimal set of tools the task requires, wrapped in middleware compiled from every applicable rule, granting only the capabilities this task's scope justifies.

Pause on that last clause, because it quietly resolves a fifty-year-old problem. Security people have preached the [principle of least authority](https://en.wikipedia.org/wiki/Principle_of_least_privilege) for decades, and almost nobody practices it, because hand-crafting minimal authority per task is economically absurd—for humans. A bootstrap agent makes minimal authority free. The harness is generated, so there is no longer any reason for the agent to possess a single capability the task doesn't need.

Inside that harness, the inner agent goes to work—and for anything beyond the small-and-revertible quadrant, its work product is not a stream of actions but a plan, reified as a program. The program is analyzed. Type-checked. Run against mocks or test tenants. Reviewed—by humans where stakes demand it, by independent adversarial agents as a matter of course.

Only then does it execute. And the program contains inference: scoped judgment calls at the joints where judgment is irreducible, subagent invocations for subtasks—which may themselves be handed to bootstrap agents that construct harnesses of their own. Turtles all the way down, with each layer's guardrails compiled by the layer above.

Watch it run twice, in two different worlds, to see that it is one architecture:

A coding task. The bootstrap agent compiles the repo's rules—test protection, commit discipline, complexity budgets—into middleware. The inner agent writes an orchestration program: analyze these modules, port these files in parallel, two reviewers per file, fix-loop until the build is green. The deliverable is code in whatever language the project demands—the machinery is fixed; the output is polyglot.

A business task. The bootstrap agent compiles company policy—refunds above a threshold require human approval; outbound email only to opted-in contacts—into middleware on the payment and email tools. The inner agent writes a typed program against the CRM, the mail system, and the analytics store: fetch the campaign data, segment the engaged leads, draft personalized outreach, queue for the approval gate. Nothing irreversible happens until the program has survived its checks.

Same skeleton. Different skin. That is the thesis: that a single recursive architecture represents the future for all kinds of agents. Not just coding agents, and not just "business" agents.

Every task leaves behind artifacts—harnesses, middleware, program-plans—and every one of them is code: versioned, reviewed, improved, reused. Your organization's operational knowledge stops evaporating into chat logs and starts compounding into libraries of tested, reusable artifacts, with tight bounds on performance, cost, and reliability.

## Fix the Machinery

A corollary about language—and a distinction the industry keeps blurring.

An agent's deliverables may need to be polyglot. A general-purpose coding agent must emit Rust, Zig, COBOL, whatever the project demands. But the agent's machinery—the reified plans, the compiled middleware, the orchestration scaffolding—need not be. Dynamic workflows writes JavaScript orchestration to produce Rust deliverables. Claude's own web interface reaches for Python as its fixed problem-solving substrate no matter what you ask about. The machinery language is a deployment decision, and fixing it pays: maximum model fluency, one containment story, one analysis toolchain.

What should that machinery language be? The criteria fall out of everything above. It must be a language the models are maximally fluent in—the training-corpus argument, and by [GitHub's own Octoverse numbers](https://github.blog/news-insights/octoverse/octoverse-a-new-developer-joins-github-every-second-as-ai-leads-typescript-to-1/), [TypeScript](https://www.typescriptlang.org/) became the most-used language on GitHub by monthly contributors in 2025, a shift GitHub explicitly attributes to typed languages making agent-assisted coding more reliable. It should be typed, because the type-checker is the error oracle that closes the generation-verification loop before effects fire—GitHub's report cites research finding that the overwhelming majority of LLM-generated compilation errors are type-check failures, which is to say: exactly the errors a type system catches in the loop, for free. It must preserve typed structure—records, enums, collections—across system boundaries, because business state flows through APIs, not shell pipes. And it must run contained: TypeScript compiles to JavaScript, which executes inside V8 isolates or [WASM](https://webassembly.org/) sandboxes where authority is granted, never ambient—so a compiled guardrail is a wall, not a suggestion.

For business agents in particular, TypeScript checks every one of these boxes today—and yes, I registered the irony of a career functional programmer and [Scala ecosystem author](https://zio.dev/) arriving at that conclusion before you did. But the deeper point is language-independent: every serious agentic system will fix its machinery substrate, and the substrate that wins will be fluent, typed, and contained.

## One Runtime

One requirement remains, and it is the one on which everything else stands or falls.

A compiled guardrail is only a guarantee if it cannot be bypassed. Middleware the agent can route around—via raw shell access, an open network, an ambient credential—is a rule in the old sense: a polite fiction. Enforcement must be a property of the execution substrate, not a decoration on top of it.

And reified programs executing against live systems make demands no traditional runtime meets. A plan that survived review must then execute exactly once through any interruption—a crash mid-refund must not produce zero refunds or two. A plan awaiting human approval must suspend—for hours or days—without burning resources and without losing its place. A thousand-node recursive structure must tolerate the failure of any node without corrupting the whole.

Here is the clearest sign that these requirements are structural, not hypothetical: the moment Anthropic moved from agent-loop to code-orchestrated agents, they immediately had to build [durability into the product](https://code.claude.com/docs/en/workflows)—dynamic workflow runs save progress as they go, resume where they left off after interruption, and coordinate outside the conversation so the plan stays on track. They built a bespoke durable workflow runtime for a single feature, in the forgiving domain, where the worst failure is a wasted run. Now generalize to domains where the worst failure is a duplicated payment, and to recursive structures where any of a thousand nested nodes can die mid-effect.

Every team that adopts this architecture will need that runtime. They will either rebuild it—badly, one incident at a time—or run on one that provides durable execution, exactly-once effects, suspension, and capability-bounded enforcement as guarantees of the substrate.

Enormous investment is currently flowing into agent frameworks that bolt hope onto runtimes which guarantee none of this. That correction will not be gentle.

Full disclosure: this is the layer I am building, with [Golem](https://golem.cloud/)—an [open-source](https://github.com/golemcloud/golem), durable runtime where agents run in capability-bounded WASM sandboxes, state survives anything, and tool effects execute exactly once. But notice that the argument has not depended on it at any point. Three independent lines—compiled guardrails need unbypassable enforcement, reified plans need transactional execution, recursive structures need durability—converge on the runtime whether or not you try Golem.

## The Future

The 101-level agent—the loop, the toolbox, the ever-growing transcript—had a good run. It demoed beautifully. It will not be how the serious agentic systems of the future are built.

The agent of the future is the whole recursive structure, not the loop at its top.

Its plans are programs, not transcripts—analyzed, tested, and reviewed before anything irreversible fires.

Its rules are compiled, not remembered—English intent translated by bootstrap agents into per-task harnesses of deterministic middleware.

And beneath all of it: one machinery language, fluent and typed and contained, on one runtime that turns its policies into physics.

None of this requires a research breakthrough. Every ingredient is shipping today, in restricted form, and the restrictions are falling in the only direction they can.

What does the successor to AGENTS.md look like when it is designed to be compiled rather than read? Who audits the compiler—what does the adversarial test suite for generated guardrails look like? What is an organization's compounding library of harnesses and program-plans worth, and who captures that value?

All excellent topics… for different posts.
