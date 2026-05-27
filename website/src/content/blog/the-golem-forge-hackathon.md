---
title: "The Golem Forge Hackathon — Build It, Deploy It, Win It"
date: "2026-05-27"
author: "Golem Cloud"
tags: ["Hackathon", "Events", "Announcements"]
slug: "the-golem-forge-hackathon"
description: "Five builders. One brief. A few days on Golem 1.5. We graded every submission on a six-axis rubric — here's what they built, who won the $2,500, and what the cohort taught us about building durable agents."
---

Golem bills itself as _"the durable agent runtime — reliability and trust by construction."_ The pitch is simple: agent logic and tools execute durably with exactly-once semantics, capabilities are bounded by the runtime, and waits, retries, and human-in-the-loop are runtime behaviors instead of state-machine code you have to write yourself.

We wanted to see what people would do with that. So we ran the Golem Forge Hackathon to find the most extraordinary thing somebody could build on Golem 1.5 in a few days.

The brief was deliberately small enough to fit on the back of a napkin — and deliberately open-ended enough to reward the people who took it as a starting point rather than a finish line.

## The brief

GolemClaw: a scaled-down OpenClaw, deployed live on Golem Cloud. One Golem app that, **per user**, spawns potentially many agents that collaborate on the user's tasks — each with its own local state, and with shared memory across the agents serving that one user.

Channel: Telegram only. LLM: Gemini Flash free tier. Tools: at least four — Reminders, Weather, Web search, Email — and feel free to go beyond. Stack: all free-tier, no paid web services. Prize: $2,500 cash to the winner, judged at Ziverge's sole discretion.

The submission requirements were minimal: a ZIP of source, a live Golem Cloud deployment, a Telegram bot handle so we could test it, and a README.

The judging criteria were intentionally a little fuzzy: "Does it actually work end-to-end on Telegram? How well does it leverage Golem's features, including durability? Quality and creativity of the tools — especially anything beyond the required four. How well memory (local + shared) is used to make the experience feel personal. Polish, reliability, surprise factor."

## How we judged

To turn that into something we could defend, we graded every submission on six numeric axes (1–10) and four pass/fail gates:

| Axis | What it measures |
| --- | --- |
| **Feature** | Breadth × depth of user-facing capabilities. Computed as `round(avg per-feature depth) + breadth bonus`. |
| **Testing** | From "no tests at all" (1) through "some unit tests" (5) to "unit + integration + CI" (10). |
| **Docs** | Human-facing build/run docs and explanations of tricky parts. AI-generated boilerplate counts against, not for. |
| **UX** | What the actual user sees in Telegram. Friendly copy, channel-appropriate formatting, inline keyboards. The inverse: raw JSON, UUIDs, HTTP status codes, stack traces, internal class names. |
| **Memory architecture** | How thoughtfully memory was designed across the per-user agents — from "no memory" (1) through "conversational memory only" (5) to "a memory system architected the way an early-stage product would actually ship" (10). Cross-user memory wasn't required, but counts as a bonus where it's done well. |
| **Golem idiomaticity** | How closely the code follows the patterns documented on learn.golem.cloud — `@endpoint` mounts, `AgentClass.get(id)` RPC, `.schedule()` for future invocations, `Secret<T>` inside `Config<T>`, and reaching for `atomically` or the saga API where the work actually spans external systems. |

| Gate | What it checks |
| --- | --- |
| **Build with Golem 1.5.3** | Does the project compile against the latest released Golem? |
| **Unit tests with Golem 1.5.3** | If tests exist, do they pass? |
| **Build-safety audit** | Does any `package.json` / `Cargo.toml` / `build.sbt` execute unsafe code at build time? |
| **Dependency reputation** | Are all dependencies from reputable maintainers? |

The rest of this post walks through the submissions one at a time so each builder can see what landed and what didn't.

We had a number of submissions, and what follows is a walkthrough of the five that made the rubric. We're presenting them alphabetically by first name, then revealing the winner and the placings below.

---

## Ajay RV

**Stack:** TypeScript SDK → WASM (Golem 1.5)
**Try it:** [@nanashi_lab_claw_bot](https://t.me/nanashi_lab_claw_bot)

### What he built

Ajay shipped a Telegram concierge built as a single TypeScript component with a deliberately layered architecture. Each Telegram chat gets its own `ChatConciergeAgent` plus six per-chat durable stores — one per concern: `ConversationStore` (transcript + compact working history + rolling summary), `ProfileStore` (name, timezone, city, emails, facts), `TaskStore`, `GoalStore`, `NoteStore`, `PortfolioStore`.

On top of those stores sit four specialist agents — `ResearchAgent` (Firecrawl-backed background research with tagged note output), `DigestAgent` (morning and evening summaries), `GoalCoachAgent` (tracking plans and stale-goal nudges), `PortfolioAnalystAgent` (lightweight portfolio nudges) — and a non-LLM `Orchestrator` that handles all cron-style automation.

End-user features include tasks with reminders, monthly recurring task templates, notes with tags and search, Firecrawl research jobs, goals with progress logs, a portfolio with holdings/watchlist/cached quotes, weather via OpenWeather, email via Resend, stock end-of-day quotes via Stooq, and timezone-aware morning + evening digests that can optionally also be emailed.

### Per-axis read

- **Feature: 7** — 24 distinct features, average depth 6.3. Breadth bonus earned (+1).
- **Testing: 1** — no tests.
- **Docs: 6** — 283-line README covering features, architecture, deploy, and slash commands.
- **UX: 5** — acceptable POC. Plain text only, no `parse_mode`, no emojis, no inline keyboards — but the copy itself is friendly, well-organised, and the error redaction (bot tokens, Resend keys, query-string keys all scrubbed before surfacing) is genuinely best-in-class.
- **Memory: 6** — cleanly-separated per-chat stores that the specialist agents read and write through; held back by no snapshotting on any of the six.
- **Idiomaticity: 8** — tied with Rahul for cleanest TypeScript Golem 1.5 hygiene.

### What's strongest

`Config<TelegramConfig>` carries **seven** `Secret<string>` fields and `.get()` is called at the point of use — the most thorough secrets-and-config story in the cohort. There is not a single `process.env` read anywhere in the agent code.

Durable scheduling is exemplary. Reminders go through `TaskStore.get(...).fireReminder.schedule(scheduleAt, id)`. The `Orchestrator` re-arms morning (09:00 local), evening (21:00 local), and an automation sweep (13:00 local) after every fire, all keyed to the user's saved timezone — including monthly task materialization that creates one instance per month due at month-end in the saved zone.

The architectural choice to split state into six focused per-chat stores (rather than one big bag) is genuinely better than the cohort norm. Each store has a focused API and a clear growth profile, and the non-LLM `Orchestrator` cleanly separates scheduling concerns from conversational ones.

### What to push past hackathon-grade

Zero tests is the single biggest gap, and our live probe showed exactly why it matters: the reminder path traps with `Component trapped: Method 'setReminder': expected 4 parameters, got 3` — the LLM-tool wiring in `chat-tools.ts` calls `setReminder(item, remindAtIso, updateKey)` (3 args) while `task-store.ts` declares `setReminder(item, remindAtIso, updateKey?, taskId?)` (4). The underlying `.schedule()` wiring itself is exemplary, but the call never reaches it. A single typed-arity test on the bridge would have caught this. The store-per-concern shape would test particularly well — each store has a small, focused API that's easy to exercise in isolation.

No snapshotting on any of the six stores means cold-start cost grows with stored history. `ConversationStore` is the standout concern: it keeps the full transcript, so it grows without bound. Adding `snapshotting: { every: N }` with hand-written `saveSnapshot` / `loadSnapshot` is a small change with a big payoff on long-lived users.

The UX is deliberately ascetic — plain text only — which keeps things robust but leaves polish on the table. Setting `parse_mode: 'HTML'` (or MarkdownV2) and adding inline keyboards for the confirm-heavy flows (email send, task complete, goal mark-done) would close most of the gap between the bot's friendly copy and what users actually see.

---

## Ashwanth Reddy D

**Stack:** TypeScript SDK → WASM (Golem 1.5)
**Try it:** [@raphel_golem_bot](https://t.me/raphel_golem_bot) · [golemclaw.netlify.app](https://golemclaw.netlify.app/)

### What he built

Ashwanth went widest. The four required tools became part of a much larger AI app that spans **Telegram + Slack + Discord** as inbound chat channels, plus Gmail OAuth for inbox polling and auto-reply, with a marketing site on Netlify on top.

The bot answers chat, manages reminders, runs web search across Tavily / HackerNews / Wikipedia / a URL reader, looks up crypto prices, manipulates spreadsheets, does GitHub OAuth + repo analysis with auto-reply, can deploy generated websites to Vercel for you, runs multi-step "missions" with progress updates, and includes a paper-trading portfolio and a sales CRM. Twenty-one distinct user-facing features delivered.

### Per-axis read

- **Feature: 7** — 21 features, average depth 5.5. Breadth bonus earned (+1).
- **Testing: 1** — no tests.
- **Docs: 5** — 489-line README plus `scripts/README.md`. Comprehensive but reads partially AI-generated.
- **UX: 5** — solid POC. Native-English copy, charming touches ("Powered by Golem — survives restarts until it fires."), and the multi-channel routing actually works.
- **Memory: 6** — richest per-user state surface in the cohort: 12 distinct slices including facts, preferences, reminders, missions, paper portfolio, sales leads, agent trace.
- **Idiomaticity: 6** — strong agent topology (per-user agents everywhere, central `rpc.ts` with `.get()` + `.trigger()`, dual-mode Telegram with webhook-primary + poller fallback, `scheduleCancelable` recurrence).

### What's strongest

The genuinely-distinct architectural move is **memory unified across channels** for a single user — Telegram, Slack, and Discord (plus Gmail OAuth for inbox polling) all converge to the same per-user agents via `channelOptIn` and `linkedChannels`. That's not a feature you can fake; it requires actually thinking about user identity as a first-class concept, separate from any one channel.

The ambition is also worth calling out on its own terms. Twenty-one features across three inbound chat channels plus Gmail OAuth, GitHub OAuth + repo analysis, a website-to-Vercel deploy pipeline, multi-step missions with progress updates, a paper-trading portfolio, and a sales CRM — that's a serious amount of product surface for a hackathon week, and it gives this codebase the widest demo story of anyone.

The agent topology backing it is clean too: per-user agents are addressed via a central `rpc.ts` typed-client module, fire-and-forget hops use `.trigger()`, scheduling uses `scheduleCancelable` with proper cancel tokens, and a dual-mode Telegram path (webhook primary, long-poll fallback) shows real production thinking.

### What to push past hackathon-grade

Secrets are the biggest opportunity. Every API key (Mistral, Google, Telegram, Slack, Discord, Vercel, OAuth client secrets) is inlined as a plaintext env value in `golem.yaml` and read via `process.env.X` from 31 files. Golem 1.5's `Secret<T>` inside a `Config<T>` is the documented path; it gives you per-environment isolation and rotation for free, and it's a small refactor away.

In the UX layer, every `<b>` / `<code>` / `<i>` you write is silently stripped by `toPlainTelegram` and the Slack/Discord channel-reply path before send, so the HTML formatting work isn't reaching users yet. Switching to channel-appropriate formatters (Telegram MarkdownV2, Slack mrkdwn, Discord md) would make a third of the copy you've already written suddenly look right — high leverage for a small change.

Finally, the welcome message and mission progress updates render the internal class names ("UserOrchestratorAgent / MissionOrchestratorAgent / GithubWorkerAgent") to first-time users. That's neat for a developer demo, but mapping each one to a friendly label ("your assistant", "your mission planner", "the GitHub helper") would make the first-run experience read the way the rest of the bot already does.

---

## Daniele Torelli

**Stack:** Rust → WASM (Golem 1.5)
**Try it:** local or self-hosted (no public bot at time of writing); admin-on-first-write — the first user to message the bot becomes the admin. We self-hosted it ourselves on Golem 1.5.3 against a throwaway BotFather token and it deployed, polled, and answered all four of our test prompts cleanly on the first try.

### What he built

Daniele built fewer features than Ashwanth and Rahul, but went deeper on each one. The bot covers the four required tools (reminders, weather, web search, email) plus web search routed through configurable providers (Brave → Serper → Wikipedia fallback), a **social-commitments workflow** with bidirectional accept / decline / done / snooze / cancel flows, a capability-based **RBAC system** with eight named capabilities (reminders, weather, search, email, llm, scheduling, commitments-create, commitments-read), per-user preferences, LLM-mode switching between Gemini and OpenRouter, and a set of admin slash-commands (`/admin whoami|users|caps|grant|revoke|make-admin|remove-admin`).

Thirteen features delivered at an average depth of 7.6 — solidly above "typical hackathon depth."

### Per-axis read

- **Feature: 8** — 13 features, average depth 7.6 (highest avg in the cohort).
- **Testing: 10** — 179 host unit tests across 12 modules, plus an end-to-end integration harness with a Python mock Telegram server, shell drivers (`e2e_local.sh`, `cloud_smoke.sh`), 12 input fixtures, and a GitHub Actions CI job that runs all of it.
- **Docs: 9** — best docs of the cohort. 671-line README with table of contents, architecture diagram, Tests-and-CI section, "Golem 1.5 features in use" section, plus a dedicated `tests/integration/README.md` explaining the mock-Telegram harness.
- **UX: 7** — best inline-UI in the cohort. Pagination (« Prev / Next »), per-item Cancel/Done buttons on every list, Accept/Decline on commitment requests, an LLM-mode switcher, share-location keyboard. `quota_denied_message` surfaces an estimated wait, not `429`.
- **Memory: 8** — per-user state plus a real singleton `SharedMemoryAgent` with user directory, `resolve_username` lookups, capability auth, and a `CommitmentAgent` keyed per-commitment (9-state machine — Draft / PendingAcceptance / Active / Overdue / Escalated / Completed / Declined / Canceled / RenegotiationRequested — with bidirectional notifications).
- **Idiomaticity: 9** — cleanest Golem 1.5 submission of the cohort.

### What's strongest

Almost everything. Webhook and poller both wired through `@endpoint` / self-scheduling (no `loop+sleep` anywhere), `#[agent_config] config: Config<AppConfig>` with `Secret<String>` fields, idiomatic `AgentClient::get(id)` + `trigger_*` RPC, durable in-memory state, and `golem_rust::atomically_async` used in exactly the one place it actually matters — wrapping the HTTP send + response-parse against the external mail provider into a single durable step.

The testing story alone is shipped-product grade: 179 unit tests, a mock Telegram API in Python, and CI that runs both. The docs read like they were written by someone who has explained this to a human before.

And the **`CommitmentAgent`** — a per-commitment durable agent that represents an actual social object with its own state machine and pings both parties — is the most architecturally interesting single feature in any of the submissions.

### What to push past hackathon-grade

UX has the most room to grow. The weather tool literally prints the WMO weather code as the integer `code 3` instead of "Partly cloudy" — a one-line lookup table is missing. Raw provider HTTP status + body leak into chat across Brave / Serper / Resend ("Resend error 429: …" reads as the body of an error message instead of being summarised). Rust enums are formatted with the debug printer (`{:?}` → `RenegotiationRequested`) in user-facing lists, which renders the variant name verbatim. `/start` clears history and dumps `/help` instead of welcoming a first-time user.

Beyond UX, the human-in-the-loop accept/decline waits on commitments are a natural fit for durable promises (`createPromise` / `awaitPromise`) — Daniele built the workflow manually with timed nudges and inline keyboards, which works, but a durable-promise version would be both shorter and more idiomatic.

None of these are bugs — they're the next layer of polish on an already-excellent foundation.

---

## Rahul Joshi

**Stack:** TypeScript SDK → WASM (Golem 1.5.2)
**Try it:** [@customminiclaw_bot](https://t.me/customminiclaw_bot) · [Mini App dashboard](https://webui-six.vercel.app)

### What he built

Rahul built the most polished single-channel product. The four required tools plus voice in/out (Groq Whisper for STT, Orpheus TTS for audio replies, toggle with `/voice on`), image vision (Telegram photos routed to multimodal LLM), a daily morning-briefing cron, a news tool, a finance tool, a recurring-reminders system (`/every 1h drink water`), a multi-step **saga-pattern booking transaction** with explicit compensation, semantic memory using Cohere embed-v4.0, an MCP server exposing several of his agents over MCP Streamable HTTP, and a full **Telegram Mini App** dashboard.

Eighteen features delivered.

### Per-axis read

- **Feature: 8** — 18 features, average depth 6.7. Breadth bonus earned (+1).
- **Testing: 1** — no automated tests.
- **Docs: 6** — 295-line README with architecture diagram, hackathon-brief coverage table, durability demo, tech stack, setup-from-scratch, project layout.
- **UX: 7** — only working inline ✅/❌ confirm-keyboard for email send in the cohort, polished Mini App with empty states and theming, voice wrapper that skips TTS for replies > 500 chars and strips markdown before reading it aloud.
- **Memory: 8** — per-user `Orchestrator` with an explicit 24-turn rolling history, plus a dedicated per-user `MemoryStore` agent that the other per-user agents read through — semantic recall via Cohere embed-v4.0 (cosine sim), with a keyword-overlap fallback when no Cohere key is configured. The closest thing in the cohort to RAG-style multi-agent shared memory for one user.
- **Idiomaticity: 8** — best TypeScript submission for Golem 1.5 hygiene.

### What's strongest

Three things stand out and none of the others have any of them.

First, the **only working saga in the cohort**: `BookingAgent` imports `operation, fallibleTransaction, Result` from the Golem SDK, defines `reserveFlight` / `reserveHotel` / `addCalendar` with paired compensations, and runs them through `fallibleTransaction(async tx => tx.execute(...))`. Not hand-rolled.

Second, the **only working inline confirm keyboard**: email sends prompt with a "📧 *Draft ready* / To: / Subject: / [preview]" message plus ✅ Send / ❌ Cancel buttons, and on tap the message is *edited in place* to `✅ {result}` or `❌ Email cancelled.` That's the textbook Telegram bot pattern and Rahul is the only one shipping it.

Third, **semantic memory via Cohere embed-v4.0** — RAG-style fact recall with cosine similarity, plus a sensible fallback to keyword overlap when no key is configured.

Beyond those three, `Secret<string>` inside typed `Config<…>` is used consistently throughout, the recurring `/every` reminders correctly re-arm via `.schedule()` inside `fire()`, and the Mini App has thoughtful empty states ("No facts stored yet. Tell me 'remember X' in chat.").

### What to push past hackathon-grade

The biggest opportunity is tests. There are none yet, and a saga-based booking flow + an inline-confirm email flow + a semantic memory store are exactly the kinds of things that benefit most from a small unit suite around them — both for catching regressions and for letting you refactor freely later.

Internal IDs also surface to users in chat — `(Draft draft-7 no longer available)`, `⏰ Set (rem-3) — fires at …`, the Brevo `messageId` in email-success messages — because `/cancel rem-3` is a real command. A tap-to-cancel inline keyboard (the same pattern you already use for email) would let you hide the IDs and keep the slash command for power users.

Raw HTTP status codes leak from weather / search / news / finance / email / briefing on the slash-command fast path that bypasses the LLM summariser; routing those through the same friendly-error wrapper the chat path uses would smooth them out.

---

## Seeta Ramayya Vadali

**Stack:** Scala.js → WASM (Golem 1.5)
**Try it:** [@golemclaw_seeta_bot](https://t.me/golemclaw_seeta_bot)

### What he built

Seeta took the brief's "shared memory" hint and built a **real cross-user directory**. The bot covers the four required tools plus iCal calendar (connect up to 3 feeds, list today's events, check slot availability), a booking system with conflict detection, a contact directory that lets you "send email to Seeta Ramayya" without knowing his email address, a daily briefing, OpenStreetMap place search, stocks, Wikipedia, a metrics dashboard, and a complete self-hosted-on-EKS deployment story.

Fourteen features delivered. Also: the only submission whose `golem.yaml` includes both a cloud profile and a self-hosted one (the latter pointed at his EKS cluster), with 13 hand-written K8s manifests in the repo.

### Per-axis read

- **Feature: 6** — 14 features, average depth 6.3.
- **Testing: 6** — 9 munit suites (`CalendarToolSuite`, `EmailToolSuite`, `IcsParserSuite`, `TelegramParserSuite`, `TelegramToolSuite`, `UserProfileToolSuite`, `WeatherToolSuite`, `WebSearchToolSuite`, `WikipediaToolSuite`). 70 tests passing.
- **Docs: 6** — 385-line README with architecture diagram, prerequisites, env vars, local setup, cloud deployment, self-hosting on Kubernetes section.
- **UX: 6** — best LLM-layer failure copy in the cohort (`"I'm temporarily at capacity. Please try again in a moment."`), tasteful emoji discipline (🟢/🟡/🟠/🔴 + ▲▼ for stocks, ⏰ for reminders).
- **Memory: 9** — **best memory architecture in the cohort.**
- **Idiomaticity: 7** — strong on the core agent shape; weaker on `Secret[T]` / `Config[T]`.

### What's strongest

The **memory architecture**. Per-user `UserAgent` holds conversation history + preferences + reminders. Cross-user memory is a two-tier sharded directory: a singleton `DirectoryAgent` holds an in-memory `nameIndex: Map[name → userId]`, then dispatches per-user reads/writes to one of **16 fixed `DirectoryShardAgent` instances** keyed by `userId.hashCode.abs % NumShards`. Each shard splits into children when it reaches 1,000 entries.

Both `DirectoryAgent` and `DirectoryShardAgent` carry `@agentDefinition(snapshotting = "every(N)")` with hand-written `saveSnapshot`/`loadSnapshot` so cold-start cost stays bounded as the directory grows. The "send email to Seeta Ramayya" feature works end-to-end via this directory — concrete proof that the architecture isn't decorative. The brief only asked for shared memory between the agents serving a single user; Seeta went further and built a cross-user piece on top, and it's the closest thing in the cohort to "a memory layer architected the way a real early-stage product would ship."

Beyond memory: the use of `scheduleAt(Datetime.afterSeconds(n))` for the daily briefing's self-rescheduling is textbook, the LLM-layer error messages are warm and useful instead of dumping a stack trace, and the K8s self-hosting story (converting Docker Compose into 13 manifests and actually running them) is a serious feat in its own right.

### What to push past hackathon-grade

The Telegram message sender never sets `parse_mode`, so every `**bold**` / `_italic_` / `[link](url)` the LLM emits renders as raw asterisks and underscores to the user. That's a one-line fix and the single highest-leverage UX change in the codebase.

Secrets all go through `Environment.getEnvironment()` as plain strings — `Secret[T]` inside `Config[T]` is the documented Scala path.

The "Help" command sends `"List all your features and what you can do for me"` to Gemini instead of being a curated string, so the help text varies each invocation and won't be formatted. There are no inline keyboards anywhere — the email tool, the booking tool, and the calendar tool would all benefit from Confirm/Cancel buttons.

`get_preferences` exposes internal keys like `_pending`, `_profile_asked`, `briefing_chat_id`, `briefing_hour` to users who ask "what do you know about me?" — pretty-printing the user-facing subset would help.

And `UserAgent`, which carries growing chat history, doesn't have snapshotting enabled — given how good the snapshotting story is on `DirectoryAgent` / `DirectoryShardAgent` / `MetricsAgent`, extending it to `UserAgent` is the obvious next step.

---

## The placings

### 🥇 1st place — Daniele Torelli

Overall **8.5**. Top of the cohort on testing, docs, idiomaticity, and feature average depth.

The cleanest Golem 1.5 codebase we received: it uses `Config<AppConfig>` with typed `Secret<String>` fields, it uses `AgentClient::get(id)` + `trigger_*` for every cross-agent call, it uses `.schedule_*` for every future invocation (no `loop+sleep` anywhere), and it uses `golem_rust::atomically_async` in exactly the one place where two side effects against an external service need to land together.

The `CommitmentAgent` — a per-commitment durable agent with its own 9-state machine (Draft / PendingAcceptance / Active / Overdue / Escalated / Completed / Declined / Canceled / RenegotiationRequested) — is the single most interesting feature anyone built. And then on top of that, 179 unit tests, a Python mock Telegram server, end-to-end shell drivers, and a CI workflow that runs all of it.

The bot itself is friendly and uses the most polished inline UI we saw (pagination, per-item Cancel/Done, Accept/Decline, share-location).

Daniele wins the $2,500.

### 🥈 2nd place — Seeta Ramayya Vadali

Overall **6.7**. Seeta gets second on the strength of his memory architecture, which is the only one in the cohort that looks like something an early-stage product would actually ship: a two-tier sharded directory with snapshotting and real cross-user lookups.

The 70-test test suite and the K8s self-hosting story round out a serious, well-engineered submission. The biggest single thing holding his score down is purely cosmetic — the Telegram sender never sets `parse_mode` — which a small fix would address overnight.

### 🥉 3rd place — Rahul Joshi

Overall **6.3**. Rahul gets third on the strength of three things nobody else in the cohort shipped: a real `fallibleTransaction` saga around his (simulated-external) flight + hotel + calendar booking, the only working in-place inline confirm-keyboard, and the only semantic memory store with proper RAG-style cosine-similarity recall feeding the rest of his per-user agents.

He's also the only one with a Telegram Mini App, and the only one who built voice-in/voice-out properly (with the smart "strip markdown before TTS" detail). What kept him from second was the testing gate — zero automated tests against a codebase this rich is a real risk; even a small unit suite around the booking saga, the confirm flow, and the memory store would close most of the gap to Seeta.

### 4th place — Ajay RV

Overall **5.5**. Ajay's submission is the broadest TypeScript entry (24 features) and ties Rahul for the cleanest Golem 1.5 idiomaticity (8) — `Config<TelegramConfig>` with seven typed `Secret<string>` fields, `.get()` at the point of use, durable `.schedule()` for reminders and for tz-aware morning/evening/automation digests, every cross-agent call via `AgentClass.get(...)`. There is not a single `process.env` read in his agent code.

The architectural choice that stood out: splitting per-chat state into six focused durable stores (Conversation / Profile / Task / Note / Goal / Portfolio) rather than one big bag of fields. Each store has a small, focused API and a clear growth profile, and a non-LLM `Orchestrator` cleanly separates scheduling from conversational logic.

What kept him from third: no tests at all, no snapshotting on the per-chat stores (`ConversationStore` keeps the full transcript so it grows without bound, which hurts cold-start cost on long-lived users), and a deliberately ascetic UX — plain text only, no `parse_mode`, no emojis, no inline keyboards. The copy is friendly and the error redaction is best-in-class, but the polish ceiling is set by the channel formatting choices.

### 5th place — Ashwanth Reddy D

Overall **5.0**. Ashwanth built the broadest product surface by a wide margin — 21 distinct features, multi-channel routing across Telegram + Slack + Discord plus Gmail OAuth, GitHub OAuth and repo analysis, a website-to-Vercel deploy pipeline, multi-step missions with progress updates. Nobody else came close on ambition.

The genuinely-distinct architectural idea is _channel-unified per-user memory_ — the same user identity across three inbound chat channels plus Gmail — and that's a feature you can't bolt on later. It's a real foundation to build on.

The opportunities for a v2 are mostly the things that come after you've nailed the surface area: adopting `Secret<T>` / `Config<T>` for the API keys currently inlined in `golem.yaml`, adding even a small unit test suite, and switching the channel-reply paths from HTML to channel-native formatting so the polish you've already written reaches users.

---

## What we learned

A few observations across the cohort that are probably useful for the next batch of Golem builders.

**Everybody got the right shape.** Every submission is per-user durable agents with cross-agent RPC. Nobody tried to write a manual `http.createServer`. Nobody tried to invent their own persistence layer.

The conceptual model of Golem — agents identified by constructor params, addressed via `Class.get(id)`, with plain in-memory fields that are durable by virtue of the runtime — landed cleanly. That's a strong signal that the docs and the SDK are doing their job.

**Some primitives never showed up.** Across all five submissions, there were zero uses of `createWebhook`, `createPromise`, or `phantomAgent`. `atomically` and the saga API each got exactly one use — both appropriate, both wrapping side effects against external (non-Golem) systems, which is what those primitives are for.

`createWebhook` and `createPromise` in particular are the right way to model human-in-the-loop or async-external-callback waits — most of the "wait for the user to tap a button" and "wait for an external system to reply" flows we saw rolled their own version using inline keyboards and per-user state. A durable-promise version is shorter and survives restarts without any extra code.

**Most submissions shipped real per-user shared memory.** The brief asked for per-user agents that share memory across one user's session, and most submissions delivered. Rahul's per-user `MemoryStore` with RAG-style recall is the cleanest example — the orchestrator, the booking agent, the daily-briefing scheduler, and the reminder agent all reach into the same per-user store. Ajay's six per-chat stores (Conversation / Profile / Task / Note / Goal / Portfolio) are another clean shape: each store is a focused durable agent and the specialist agents read and write through them.

Seeta went a step further and built a **cross-user directory** on top — a singleton routing layer with sharded children and snapshotting, sized to grow unboundedly. Cross-user wasn't required by the brief, but it's a clean architectural extension and arguably the most "shipped-product-shaped" memory layer in the cohort.

**`Secret<T>` is the cleanest split in the cohort.** Three of the five submissions (Daniele, Rahul, Ajay) declare every API key inside a typed `Config<T>` with `Secret<T>` fields; two (Seeta, Ashwanth) hard-code API keys as plaintext env values in `golem.yaml` or read them through `process.env`.

The `Secret<T>` inside `Config<T>` pattern documented at learn.golem.cloud is a five-minute change that gives you per-environment isolation and live rotation, and the difference in code quality is immediate. If you're starting a Golem project from scratch tomorrow, set up your `Config` type first.

**Tests pay off disproportionately.** The two submissions with real test suites (Daniele's 179 + integration harness, Seeta's 70) were also the most polished.

The submissions without tests had subtle UX bugs that a test would have caught immediately (a `chatId` overwrite that broke replies entirely, an HTML formatter silently stripped before send, a confirm-button callback that dropped the draft on stale callbacks).

---

## Try them yourself, then come build with us

If you'd like to play with what the cohort built, here are the live links that are still up at time of writing:

- **Daniele Torelli** — Rust; no public bot at the moment, but the source builds and runs cleanly locally or self-hosted
- **Seeta Ramayya Vadali** — [@golemclaw_seeta_bot](https://t.me/golemclaw_seeta_bot)
- **Rahul Joshi** — [@customminiclaw_bot](https://t.me/customminiclaw_bot) · [Mini App](https://webui-six.vercel.app)
- **Ajay RV** — [@nanashi_lab_claw_bot](https://t.me/nanashi_lab_claw_bot)
- **Ashwanth Reddy D** — [@raphel_golem_bot](https://t.me/raphel_golem_bot) · [golemclaw.netlify.app](https://golemclaw.netlify.app/)

If you want to build the _next_ Golem app, the homepage challenge is the right place to start: _"Crash your first agent in five minutes. Watch it come back."_ Head to [learn.golem.cloud](https://learn.golem.cloud) — the Quickstart will have you running an agent locally in under ten minutes, and the How-To Guides cover everything we found underused in this hackathon: webhooks, durable promises, scheduling, atomic blocks, sagas, secrets, snapshots.

Golem's pitch is _agents that never fail_ — exactly-once execution, capability-bounded sandboxes, suspension and retry as runtime behaviors instead of state-machine code you have to write. This cohort showed what happens when builders take that pitch seriously: per-user durable agents, cross-user directories that survive restarts, sagas with real compensations, schedulers that fire across deploys. There's a lot more in the runtime than four days lets you exercise.

Congratulations to Daniele, Seeta, Rahul, Ajay, and Ashwanth. And to everyone reading this who's about to start their first Golem app: it's a really good time to start. Come build something extraordinary.
