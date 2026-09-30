---
title: "Making decisions with Jev in Golem"
date: "2026-09-29T18:00:00Z"
author: "Daniel Vigovszky"
tags: ["Engineering Articles", "AI Agent", "Jev"]
slug: "making-decisions-with-jev-in-golem"
description: "Build a durable log incident pipeline that uses Jev to classify events and route them between Golem agents."
---

## Introduction

Two weeks ago [TypeSafe AI introduced Jev](https://typesafe.ai/blog/introducing-system-one-models-and-jev) and for a while everyone seemed to talk about it, showing more and more interesting use cases. In this post I'm not trying to find the most exciting new application for this AI model; instead I'm going to show how easy it is to introduce it to a [Golem](https://golem.cloud) application (just like into any other TypeScript project), and use this opportunity to talk more about Golem. Why would you use Jev in a Golem application? Why would you use Golem for something like the demo we will look into?

## The use case

**Jev** is basically a fast general purpose classifier - and it can be used for arbitrary tasks without training it first. **Golem** is a platform for running agents - where agents are stateful, can communicate with each other, and can use LLMs or other AI (or non-AI) services in a fault tolerant and very safe, sandboxed environment.

A simple use case I came up with for this experiment is to use **Jev** as a filter and router among Golem agents. We are going to build a log ingress that maintains a sliding window of logs, and for each log line it asks Jev to classify it based on a configurable set of categories (for example security incident, service outage, data loss, performance degradation or none of these). Then we wait a bit more to acquire the surroundings of a matched log, and we forward it, together with a configurable amount of surrounding logs, to one or more other agents corresponding to the categories the classifier selected. This per-category agent then summarizes the event using an LLM and stores it in an event log.

![Conceptual flow from HTTP log ingress through Jev classification to per-category incident registries](/blog-images/jev-golem-concept.svg)

## Building it

In the past I really enjoyed explaining step by step in blog posts like this how to build something using a given technology. I don't think it makes too much sense anymore - nobody is going to follow it step by step, and coding agents can learn the _how_ from the source code (Golem itself is open-source) and the [provided skills](https://blog.vigoo.dev/posts/golem15-part9-skills/).

In this particular case I just took the last pre-release version of Golem (**Golem 1.6** is to be released within a few weeks!) and created a new Golem application using the [Effect.ts](https://effect.website) template provided by the `golem` CLI:

```
$ golem new jev-demo
? Select languages:
  TypeScript
> Effect
  Rust
  Scala
  MoonBit

? Select templates:
> [x] effect: A simple Effect agent implementing a durable counter
  [ ] effect/streaming: A runnable Effect Durable Streams agent with HTTP routes, external streams, and forks
...
Finished applying template(s) [OK]
```

Then I just gave a prompt similar to the following to a coding agent:

> Let's remove the counter example and build a demo. First I want you to understand how Jev can be used (https://typesafe.ai) and in addition to that, how to use it from within Effect.ts.
>
> What I want to build is the following:
>
> - We will have a durable ingress agent that continuously receives log lines (structured json documents). The agent's state will be a circular buffer holding always just the last 100 lines (configurable through agent config).
> - We will also have an ephemeral facade agent that takes a plain-text log and parses timestamp / content out of it and forwards to the durable ingress
> - Every time the durable ingress gets a new line, we will call Jev to classify it among a number of watched preconfigured categories (+ none of them). If there is a match, we remember it and whenever it gets to the middle of the circular buffer OR a given TTL expired, we forward it and the whole surrounding circular buffer to an inner durable agent that's keyed by the category name.
> - Maybe we should use a second round of jev-based filtering on the buffer before we send it and keep only the ones it flags. Not sure
> - The per-category actors receive one "incident" with the surrounding logs, and run an LLM (using effect.ai) to summarize the incident based on the logs, then stores it in its own state keyed by timestamp. It should also have a method to query the stored incidents. As these queries cannot interleave with the LLM generation, we need some trick here, maybe use the read-only method feature to cache results, or spawn a sub-agent. Let's discover what's best.
> - All features must be exposed via HTTP
>
> Let's not use stream types / durable streams yet - we want to demonstrate that capability separately.

I gave some pointers to it about how I imagine our architecture but intentionally did not give it too much detail - I was curious what kind of solution it would come up with for the interleaving problem I mentioned, for example. Using read-only methods, that I hinted in the prompt, is actually not a good solution in this case but I wanted to see how it answers.

From this the agent created, deployed and tested a fully working Golem application implementing my use case correctly, with basically no further interaction needed. To test it I asked the agent to download some example log archives, run Golem locally, send sections of this downloaded archive to the ingress and then examine the various incident categories through our application's HTTP API.

## Architecture

So instead of focusing on how it built it, let's start from the end result. The resulting architecture looks like this:

![Architecture of the Jev and Golem log incident processing demo](/blog-images/jev-golem-architecture.svg)

### Understanding the components

The left section contains three categories of the public HTTP API we implemented:

- It is possible to post a plain text log line
- Or post a structured log JSON
- A few GET endpoints provide access to the collected incident summaries

In Golem we don't have to write HTTP endpoint definitions and routing separately (although we can for custom cases) - they can be directly defined on the **agents** that are going to be serving the requests.

The middle section contains our agent types, with arrows representing the message flow between them.

The right section represents the 3rd party AI providers we are going to connect to - in this example it's TypeSafe AI's Jev and OpenAI.

#### Agent types

Before talking about all the 4 agent types on the above diagram, take a note of the labels on them: **ephemeral** and **durable**.

**Ephemeral agents** are stateless, one-shot entities, perfect for example for request handlers of an HTTP API. They are fast and an arbitrary number of them can run in parallel, but they are not fault tolerant - they are designed for short-living, retry-able units of work.

**Durable agents** are the default, and they represent one of Golem's main powers. Durable agents can run arbitrarily long and they survive crashes, redeployments, scaling and so on. Durable agents support _idempotency keys_ and automatically provide agent-to-agent communication with _exactly-once_ calling semantics - if an agent A calls an agent B, you can be sure that it is going to happen, and the call is not going to reach B twice even during outages. Another nice property of durable agents - which enables the fault tolerance we've just talked about - is that they can be _suspended_ when they are idle. The runtime can remove them completely from memory, and still they are ready to be restored (by a new invocation, by a timer, etc) and they just continue running from where they were.

An interesting feature emerges from the above properties - if it can be suspended and resumed, and "survives anything" (crashes, redeployments, etc...), we can simply store arbitrary state in an agent's memory (in regular variables) and there is no need to synchronize data into a database. This is something we are going to take advantage of in our demo!

Another important property of durable agents is that their invocations are processed _sequentially_ and they cannot overlap. Maybe surprisingly this no-overlap constraint also includes promises. The invocation is only considered done when all promises have been resolved (or cancelled). This is a limitation that we probably will resolve in future versions. I actually like it though, as it also makes it much easier to understand how the whole system is executing. As we will see, this limitation requires some attention in some cases to make sure things are not blocked in the invocation queues.

#### CategoryIncidents agent

Let's start with the **CategoryIncidents** agent. One **instance** of this agent will represent the incidents of **a given category** for **a given log source**.

For example if we have a log source `payment-service` and three categories (`security-incident`, `data-loss`, `service-outage`), then the following **agent IDs** are going to identify different instances of the same type:

- `CategoryIncidents("payment-service", "data-loss")`
- `CategoryIncidents("payment-service", "service-outage")`
- `CategoryIncidents("account-service", "service-outage")`

The agent itself is going to be a simple agent, not having any AI enabled functionalities - just storing a list of incidents in its own state, and providing HTTP bindings to query that. When using the Effect.ts SDK for Golem, we define our data types used in our agents (whether it's on the public API or to store the state, etc.) using **Effect Schema**:

```typescript
// .. more domain data types defined with Effect Schema ..

export const Incident = Schema.Struct({
  context: IncidentContext,
  status: Schema.Literals(["pending", "summarized", "failed"]),
  summary: Schema.NullOr(Schema.String),
  error: Schema.NullOr(Schema.String),
});

export type Incident = typeof Incident.Type;

export const CategoryState = Schema.Struct({
  source: Schema.String,
  category: Schema.String,
  incidents: Schema.Array(Incident),
});

export type CategoryState = typeof CategoryState.Type;
```

Our agent's state will store the `source` and `category` (the pair that identifies the agent) and an array of `Incident`s, each storing the log lines belonging to the incident, and the LLM-based summarization (to be explained later).

The agent itself defines not only its name, identity and invokable methods, but also the public HTTP API it is bound to:

```typescript
export const CategoryIncidents = defineAgent({
  name: "CategoryIncidents",
  description: "Durable incident store for one source and watched category",
  mode: "durable",
  id: { source: Schema.String, category: Schema.String },
  http: Http.mount("/categories/{source}/{category}"),
  snapshotting: Snapshot.define({
    schema: CategoryState,
    policy: Snapshot.policy.everyN(10),
  }),
  methods: {
    list: method({
      input: {},
      success: Schema.Array(Incident),
      http: [Http.get("/incidents")],
    }),
    receive: method({
      input: { incident: IncidentContext },
      success: Schema.Boolean,
    }),
    complete: method({
      input: { incidentId: Schema.String, summary: Schema.String },
      success: Schema.Boolean,
    }),
    fail: method({
      input: { incidentId: Schema.String, error: Schema.String },
      success: Schema.Boolean,
    }),
  },
});
```

This is just the _definition_ of our agent - we need to actually _implement_ each method as well. It's pretty simple, just storing data in our agent's state, backed by an Effect.ts `Ref`, for example:

```typescript
CategoryIncidents.implement<Ref.Ref<CategoryState>>({
  init: ({ source, category }) => Ref.make<CategoryState>({ source, category, incidents: [] }),
  methods: (state) => ({
    // ...
    list: () => Ref.get(state).pipe(Effect.map(({ incidents }) => incidents)),
  }),
  snapshot: Snapshot.ref<CategoryState>(),
);
```

This agent demonstrates how the durability feature can be used to build agents corresponding to domain entities, simply storing their data in their own state. The _snapshotting_ feature (appears in both the agent definition and the implementation) is optional - Golem can preserve the agent's state even without it, but our agent was right to use it as it makes recovery from suspended state much faster.

#### SummarizationJob agent

The behavior we want from `CategoryIncidents` is that when it `receive`s a new incident, it uses an LLM to summarize it before storing the final incident in the incident list. (We can still store the non-summarized version in the incident list temporarily, so there is no delay in incident queries). The summarization, however, is a slow operation involving sending one or more requests to a 3rd party AI API. This is one of the cases where the **sequential, non-overlapping invocation processing** has to be dealt with. If we simply start the (async!) summarization in the `receive` call, our agent is not going to process any other invocations until it is fully done, including the summarization. For subsequent incoming `receive` calls this is not a problem - we want to process them one by one anyway. But it also means our agent cannot create responses for the incident listing endpoint as it's bound to invoking the `list` method!

Our coding agent used a simple trick to avoid this problem - spawning another agent for each `receive`ed incident, that runs in parallel to our `CategoryIncidents` agent and just sends back the summarization result to it when it is done.

This agent shows us a couple of new features of Golem.

First of all, the LLM call requires an API key (and an API URL and model name). We can define this required configuration using Golem's `defineConfig` function:

```typescript
export class SummarizationConfig extends defineConfig("SummarizationJob.Config", {
  openAiApiUrl: Schema.String,
  openAiModel: Schema.String,
  openAiApiKey: Schema.Redacted(Schema.String),
}) {}
```

Then in the _agent definition_ we just add it as a property:

```typescript
defineAgent({
  // ...
  config: SummarizationConfig,
  // ...
});
```

and in the _agent implementation_ we can just get it from our Effect's dependencies:

```typescript
const config = yield * SummarizationConfig;
```

Then the implementation of our summarization agent's `run` method just uses the `@effect/ai-openai` package to call OpenAI with the configured URL, token and model, with a prompt constructed from the log lines passed to `run`.

The `run` method is defined like this:

```typescript
defineAgent({
    // ...
    methods: {
      run: method({
        input: { incident: IncidentContext },
        success: Schema.Void,
      }),
    // ...
});
```

And the interesting feature that `defineAgent` provides is a fully type-safe way to **invoke an agent from another agent**. Here is how `receive` in the `CategoryIncidents` agent's implementation triggers the summarization:

```typescript
const job =
  yield *
  SummarizationJob.client.get({
    source: current.source,
    category: current.category,
    incidentId: incident.id,
  });
yield * job.run.trigger({ incident });
```

`get` means "get or create" - the parameters passed are the constructor parameters for our `SummarizationJob` agent, and they uniquely identify an agent instance. This means there is going to be **one summarization job per incident**.

Then we **trigger** the run method, passing the incident. Triggering means we put the invocation in the other agent's invocation queue (with guaranteed delivery) and then continue - so we are not blocking our sender agent with the summarization job.

When the summarization is done, the other agent uses the same technique to send back the summarization result:

```typescript
const categoryStore =
  yield *
  CategoryIncidents.client.get({
    source: current.source,
    category: current.category,
  });
yield *
  categoryStore.complete({
    incidentId: current.incidentId,
    summary: result.text,
  });
```

Here our agent wrote `.complete` and not `.complete.trigger` - this is the variant that awaits the remote invocation's completion. In this particular case it is not making a lot of difference, but helps a little with debugging our application's state - the summarization job's **status** field (in its own state) only becomes `completed` when it knows the `CategoryIncidents` agent already received the summary.

##### Read-only methods

In the agent prompt I mentioned read-only methods to the agent as a possible way to deal with the problem of not being able to return an incident list while summarization is running. Read-only methods cache their values (with some configurable expiration) and if the cache is hot, it requires no actual invocation to serve the HTTP request bound to them.
This is a useful optimisation technique but it would not solve our problem reliably here - it is not guaranteed that the cache is valid, so from time to time external requests trying to query the incident list would block on ongoing summarizations, if we would not have this separate `SummarizationJob` agent.

#### LogIngress agent

The `LogIngress` agent is where we integrate **Jev** in our application. It is a durable agent, with one instance per log source. The agent's state is a sliding window of observed structured log lines, and observed incidents. When we detect an incident with **Jev** we are not immediately sending it to our `CategoryIncidents` agent. First we wait a bit for follow-up log lines. We send it either when the classified log line is in the middle of our buffer, or after a configurable TTL (to make sure we handle the matches even if no follow-up log lines are arriving).

This is an example of having a more complicated in-memory state, where Golem's **durable execution** guarantees make this very easy to implement. Imagine writing this on a traditional runtime. We would have to persist the log lines, deal with throwing away old ones (or choose a database that supports such a construct), we would separately have to persist the candidates until we are ready to send them to the incident store agents, and so on. With Golem we don't have to do any of that - just simple in-memory state, guaranteed to never get lost:

```typescript
const PendingMatch = Schema.Struct({
  id: Schema.String,
  category: Schema.String,
  probability: Schema.Number,
  matchedAt: Schema.String,
  matchedSequence: Schema.Number,
});

export const LogIngressState = Schema.Struct({
  source: Schema.String,
  nextSequence: Schema.Number,
  lines: Schema.Array(SequencedLogLine),
  pending: Schema.Array(PendingMatch),
});
export type LogIngressState = typeof LogIngressState.Type;
```

Sending a pending incident to the other agent works with the same generated type-safe client interface as the one we've already seen. The interesting part of this agent is how we use **Jev** for routing among the agents, and how we deal with the TTL.

To ask **Jev**, we can use the official Effect.ts library `"@effect/ai-typesafe"`. We store the classifier configuration itself in the agent's configuration:

```typescript
export class LogIngressConfig extends defineConfig("LogIngress.Config", {
  bufferSize: WitTypes.Int32,
  contextTtlSeconds: WitTypes.Int32,
  jevApiUrl: Schema.String,
  jevModel: Schema.String,
  categories: Schema.Array(WatchedCategory),
  typesafeApiKey: Schema.Redacted(Schema.String),
}) {}
```

We've already seen this where we specified the OpenAI API keys, model and URL. The nice thing about Golem's configuration support is not only that it provides these values through the Effect.ts SDK as a layer; it also **ensures** that all the configuration is present at deploy-time, and guarantees that it is type safe. In addition to that, secrets (`Schema.Redacted(...)` in Effect.ts) are treated as secrets on the Golem deployment level too. They are redacted everywhere, and Golem provides special APIs and CLI commands to **rotate them** without redeploying your application.

In the `golem.yaml` (our application's Golem specific manifest) we can specify our agent's default configuration and the initial values for the secrets (_initial_, because they can be rotated later):

```yaml
agents:
  LogIngress:
    config:
      bufferSize: 100
      contextTtlSeconds: 30
      jevApiUrl: "https://api.typesafe.ai/v1"
      jevModel: "jev-latest"
      categories:
        - name: security-incident
          description: "Authentication attacks, unauthorized access, suspicious activity, or security policy violations"
          threshold: 0.65
        - name: service-outage
          description: "A service is unavailable, unhealthy, repeatedly crashing, or failing requests"
          threshold: 0.65
        - name: data-loss
          description: "Data was deleted, corrupted, lost, or cannot be recovered"
          threshold: 0.65
        - name: performance-degradation
          description: "Latency, saturation, resource pressure, or throughput is materially worse than normal"
          threshold: 0.65
# ...
secretDefaults:
  local:
    typesafeApiKey: "{{ TYPESAFE_API_KEY }}"
    openAiApiKey: "{{ OPENAI_API_KEY }}"
```

The above secret syntax tells Golem to use the `TYPESAFE_API_KEY` and `OPENAI_API_KEY` environment variables from the machine we are deploying from.

Getting the above config from our agent's dependencies and using the **Jev** API is straightforward:

```typescript
const config = yield * LogIngressConfig;
// ... extract decisions and other properties from config
const definition = Decision.make({ input: LogDecisionInput, decisions });
const clientLayer = TypeSafeClient.layer({ apiKey, apiUrl }).pipe(
  Layer.provide(FetchHttpClient.layer)
);
const modelLayer = TypeSafeDecisionModel.layer({ model }).pipe(Layer.provide(clientLayer));
const response =
  yield *
  DecisionModel.decide(definition, {
    input: {
      timestamp: line.timestamp,
      content: line.content,
      attributes: Record.fromIterableWith(line.attributes, ({ key, value }) => [key, value]),
    },
  }).pipe(Effect.provide(modelLayer));
```

We can call `decide` for each incoming log-line and get back probabilities of matching each configured category. We convert this to a list of category matches - if the list is empty, it did not match _any_ of our classifier categories. Then we store the matches in our agent's in-memory state, and check if any previously pending items are now ready to be sent (when they are in the "middle" of our sliding window). Sending them is trivial - just one more agent-to-agent call using the typed client that comes free from `defineAgent`.

The only new thing here is how to deal with the TTL; what if we have a match, but then nobody sends a new log line ever again - we would have the pending match pending forever. Spawning a timer purely in the JavaScript (Effect.ts) world is not an option because of the requirement that invocations cannot overlap - if we still had a pending promise at the end of `receive`, new log lines would never arrive. But we also use the _clients_ defined by `defineAgent` to **schedule** an invocation in the future, and we can even do that on ourselves!

Here is what this looks like in code:

```typescript
const self = yield * LogIngress.client.get({ source });
for (const match of newPending) {
  yield * self.flushPending.schedule(scheduledAt(now + ttlSeconds * 1_000), { matchId: match.id });
}
```

#### PlainTextLogFacade agent

The last agent from our diagram is an **ephemeral agent**. It has no state, and Golem spawns a fresh instance for each invocation - in our case, for each incoming POST request on the plain-text log ingress endpoint.
In our demo there is no serious reason to have this log parsing in its own ephemeral agent - the reason I wanted to have it is to demonstrate how ephemeral agents can be used as the implementation of HTTP APIs while they can still communicate with the underlying durable agents.
We can argue for using ephemeral agents as the plain-text parsers to avoid the computation cost of parsing the log lines on the `LogIngress` agent itself.

## Conclusion

Adding **Jev** based decisions to a Golem application is trivial. You can use it to control agent loops, make routing decisions between multiple agents, and so on.
This post used the _Effect.ts SDK_ for Golem, but you can also write Golem applications in plain TypeScript, in Rust, Scala and MoonBit.

The above example was built using a pre-release version of **Golem 1.6**. The final version is coming out soon. We did not really use any 1.6 features in it though - everything described here is possible using the current stable Golem 1.5 release, except that its Effect.ts SDK depends on an _older_ version of Effect.ts with no `@effect/ai-typesafe` package, so we would have had to write our Jev client for ourselves using Effect's HTTP client API.

The whole demo is available [on GitHub](https://github.com/vigoo/jev-demo).
