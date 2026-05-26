---
title: "Everything You Must Know About AI Agents: Hype vs. Reality"
date: "2025-10-22"
author: "Afsal Thaj"
tags: ["Industry Articles"]
slug: "everything-you-must-know-about-ai-agents-hype-vs-reality"
originalUrl: "https://golem.cloud/post/everything-you-must-know-about-ai-agents-hype-vs-reality"
---

This blog covers just about everything that you need to be aware of AI agentic systems, both conceptually and technically.

## Good to know before you know!

If you're new to system design, it's important to first understand its role in solving real-world engineering problems. System design encompasses foundational concepts such as distributed **computing,** CAP**,** state persistence, anti-corruption layers, scalability, as well as the many strategies engineers develop over time to achieve resilient, fault tolerant and reliable engineering solutions**.**

The reason to understand this now is to clearly see the boundary between what agentic patterns discussed below and what falls under classical system design challenges. This perspective helps you avoid conflating the two, allowing you to appreciate agentic systems for what they truly offer, without getting lost in rethinking what's already well established in traditional system design.

## Beyond Hype: Understanding AI Agents Without Losing Sight of Engineering

Some may ask, "What's new about these AI agents?" while others might wonder, "Why should I even care about coding, system design, or established software engineering paradigms anymore?" Both perspectives miss the point. One is trapped by confirmation bias, holding on to familiar ideas and dismissing new paradigms, while the other falls prey to novelty bias, chasing trends without grasping the underlying principles.

This is exactly why I wrote this — to highlight the importance of recognizing both biases and finding a balance between innovation and established engineering practices.

## What is an Agentic System ?

An agentic system is a software system that internally uses large language models (LLMs) to solve problems in a goal-directed, controlled, and reliable manner.

But it's not just a simple delegation of the problem back to AI. Instead, an AI agent is a careful orchestration of interactions with the LLM in a way that it produces refined, context-aware, deterministic, and human-friendly outputs. As a simple example, an agent may break a single user query into multiple structured steps, ensuring that each step is precise, fast, and interpretable.

For example, consider an agent designed for hikers. When a user asks: "is it good time next week to hike Cradle mountains in Tasmania?", the agent should respond with a friendly response that is reliable.

A naive approach would be to pass the entire query directly to the LLM. But a robust agentic approach would work in multiple steps:

1. Extract relevant entities: Use the LLM to identify the location ("Cradle Mountains") and the temporal reference ("next week").
2. Fetch factual data: Call a predefined function, ***`get_weather(location, date_range)`***, to retrieve weather data for the specified place and time.
3. Generate human-friendly guidance: Use the LLM again, combining the extracted query context and the factual weather data, to produce a response such as: "The forecast for Cradle Mountains next week is sunny and mild — perfect for a hike. Don't forget your sunscreen and comfortable hiking shoes!"

Additional functions, like ***`get_suggested_gears`*** or ***`get_trail_conditions`***, can be integrated into the agent, allowing it to provide a comprehensive and actionable recommendation rather than just a text answer.

In short, an agent is not merely a conduit to an LLM. It is an orchestrator that combines domain-specific logic, structured function calls, and multiple LLM interactions to reliably solve user queries in a deterministic and human-friendly way.

## AI Agents from a Senior Software Engineer's Perspective

If you are an experienced software engineer, the concept of an AI agent often prompts the question: "How is this different from regular code (or system) that orchestrates tasks?" The boundary is indeed still unclear. However, being precise about patterns specifically tailored for AI agents is important, which is why a variety of agentic frameworks have emerged.

## Agent frameworks and hybrid systems like Golem

Each design pattern (to be discussed) in agentic space can be implemented using existing frameworks like LangGraph, ADK, Crew AI and so on, or you can build your own system, manually integrating the AI components into your existing design. Alternatively, hybrid frameworks like Golem allow you to write code in your usual style, without heavy DSL intervention, while still being optimized for AI-driven tasks with batteries included. The hybrid approach separates classical system design concerns (such as scalability, durability of the data involved, state persistence etc) from agentic concerns, letting you focus solely on business logic of the agent.

If you need a code level comparison of the established frameworks and Golem, here is one: [https://medium.com/@afsal-taj06/how-golem-stands-out-in-the-world-of-ai-agents-7747c275ce94](https://medium.com/@afsal-taj06/how-golem-stands-out-in-the-world-of-ai-agents-7747c275ce94).

## Why Hybrid Systems Like Golem Are the Smart Choice for Agents

To repeat, at its core, an agentic system is still just a regular software system — but with connections to AI. The lack of clarity around what defines an "agentic" system often shows up in the frameworks developers choose. Many frameworks try to turn every problem — both domain-specific and system design — into rigid agentic patterns, forcing developers to learn concepts that may become irrelevant once trends shift.

For example, running tools concurrently as part of an agent (or running agents themselves in parallel) is something you already know how to handle in any programming language. Many frameworks provide DSL functionalities to solve this, but in doing so, they turn every system-level or domain-level challenge into a problem of the framework itself, adding unnecessary complexity.

Hybrid systems like Golem solve this elegantly. You write code as you normally would, without any AI-agentic knowledge. When you want to incorporate AI, you simply bring in Golem-AI as a dependency. Golem handles common system concerns — state persistence, independent scaling of agents, and reliability — allowing you to focus purely on domain-specific logic.

Compared to frameworks like LangGraph, Golem strikes the perfect balance: it lets you leverage AI when needed, while keeping your system robust, maintainable, and grounded in classical software engineering principles.

## Agent Design Patterns

With all that out of the way, let's try and understand what are the common design patterns in an Agentic system. Much of this is taken from a 407 page explanation on [agentic patterns](https://docs.google.com/document/u/0/d/1rsaK53T3Lg5KoGwvf8ukOUvbELRtH-V0LnOIFDxBryE/mobilebasic), but in my own words.

If you are following hybrid approach (be it Golem or some other way), all you need to be aware is understanding the meaning of these patterns. There isn't a need to know how each of these patterns are achieved in a specific framework. Consider this to be a summary of what you "should" know!

### Prompt chaining

Prompt chaining is the process of breaking a single user query into multiple smaller prompts and sending them to the LLM one after another. The output of one LLM call becomes the input for the next. This approach helps your agents behave more predictably and consistently.

For example, in an agent designed for hikers, the system might first ask the LLM to extract the location and date from the user's query, then fetch the weather for that location, and finally generate a human-friendly response combining both pieces of information. By chaining these prompts, the agent produces a more controlled and reliable answer than if it tried to handle everything in a single step.

### Routing

In agentic systems, routing determines which function, module, or sequence of steps an agent should take to handle a user query. Several approaches exist, each with its own trade-offs:

1. **Rule-based routing**: The simplest approach, using straightforward if-else conditions to decide the route.
2. **Embedding-based routing**: The prompt is converted into a vector (embedding), and each possible function (which can itself be a prompt chain) also has an embedding. The agent chooses the route based on which function embedding is closest to the prompt embedding.
3. **Machine learning-based routing**: A supervised classification model (example: SGD) is used to determine the route, leveraging patterns learned from historical data. This may involve a separate training and tuning of the machine learning models and not LLM
4. **LLM-based routing**: The LLM itself is asked to decide the route. While flexible, this approach is usually less deterministic.

At this point, you might wonder: Aren't these just regular software engineering problems? To an extent, yes. One distinction is that AI elements — like embeddings, ML models, or LLMs — are actively used to make routing decisions, making them specific to agentic systems rather than traditional software. In frameworks like Google ADK or LangGraph, there is a specific way to encode this routing. However to learn them or not, depends on how much of your system is deeply coupled into these frameworks. As mentioned before, hybrid patterns make sense to me personally.

### Tool Orchestration

Tool orchestration is a pattern specific to agentic systems. The idea is simple: the agent asks the LLM which tool to use and with what input, and then the framework executes that function or tool accordingly. Unlike routing patterns — which determine which code path or function to follow — tool orchestration focuses on _dynamically deciding which external capabilities or APIs to invoke_ based on the context of the user query.

For example, an agent might have access to multiple tools like ***`get_weather()`, `calculate_hike_difficulty()`, or `fetch_trail_conditions()`***. When a user asks, "Is it safe to hike the Cradle Mountains next week?", the LLM can analyze the query and decide that it needs to call both ***`get_weather()`*** and ***`calculate_hike_difficulty()`.*** The framework then executes these tools in the correct order, collects the outputs, and passes them back to the LLM to generate a final, human-friendly response.

### Reflection

The reflection pattern introduces a feedback loop within an agentic system. An evaluator — such as an agent dedicated to reviewing outputs or assessing performance — analyzes the agent's actions or responses and provides feedback. This feedback can then be used to refine subsequent decisions, improve accuracy, or guide better reasoning.

In short, reflection allows agents to **learn from their own outputs** and continuously improve, making the system more adaptive and reliable over time.

In some cases, there are two specific agents, such as "production agent" and an "evaluator agent", and in some other cases, its more or less self evaluation (Self reflection).

### Parallelization

The parallelization pattern involves running multiple agents, tool calls, or LLM processes concurrently or even in parallel. Conceptually, there's nothing particularly unique here — this is a standard software engineering problem. Developers who understand concurrency and parallelism should be able to handle it in their preferred language without being forced to learn framework-specific mechanisms.

Frameworks like LangGraph or ADK may provide specialized functionalities to achieve parallel execution, but they are often tied to their runtime or SDK and can be limited by the language's underlying execution model. For example, in Python, LangChain offers combinators for parallel tasks.

Here is an excerpt from this [book](https://docs.google.com/document/u/0/d/1rsaK53T3Lg5KoGwvf8ukOUvbELRtH-V0LnOIFDxBryE/mobilebasic) that talks about an interesting detail of LangGraph's specific parallel combinators.

> _Internally, however, it relies on `asyncio`, which provides concurrency rather than true parallelism. On a single thread, `asyncio` switches between tasks when one is idle, giving the appearance of parallel execution, but all code still runs on one system thread due to Python's Global Interpreter Lock (GIL)._

### Planning

This pattern is specific to the agentic world and focuses on generating steps to follow dynamically. Unlike many of the patterns discussed earlier, which rely on pre-defined specifications or workflows, planning pattern is more about using the agent itself to generate the specification.

The need for this arises from the inherent unpredictability of real-world scenarios. Consider a robot that needs to behave appropriately in a variety of situations. You cannot predefine every possible sequence of actions because the combinations of situations are effectively infinite.

By generating steps dynamically, the agent can leverage AI to determine the best course of action in real time, without requiring you to build your own LLM models.

In short, dynamic step generation unlocks the full potential of AI in agentic systems, allowing agents to handle **complex, ever-changing environments** in a way that static workflows cannot.

A good example from the book:

> _Within robotics and autonomous navigation, planning is fundamental for state-space traversal._

Frameworks like [CrewAI](https://www.google.com/search?client=safari&rls=en&q=crewAI&ie=UTF-8&oe=UTF-8) are good to handle all these. Google's deep research is an example of an agentic system that makes use of Planning.

### Multi Agent Pattern

While many of the patterns discussed earlier focus on a single agent (except for parallelism), real-world agentic systems often involve multiple agents working together. Communication between these agents follows specific protocols, allowing the system to decompose a problem and scale efficiently. Each agent can handle a portion of the problem independently, running on the same node or across different machines depending on the runtime. This is why agentic systems share similarities with the actor model and distributed systems.

Frameworks like Golem simplify this by automatically managing agent separation and scaling. You simply write code that interacts with another agent, and the framework ensures the agents run independently and scale as needed. In other systems, handling this separation might require more manual effort.

The multi-agent pattern can be further subdivided:

1. **Network Agents**: Agents communicate reliably with each other, each handling its own part of the problem.
2. **Multi-Agent with Supervisor**: A supervisor agent orchestrates other agents, similar to Zookeeper managing workers. This introduces a potential single point of failure.
3. **Nuanced Supervisor (I call it)**: A supportive supervisor provides tools and assistance to agents without directly controlling them — more like a good leader.
4. **Hierarchical**: Layers of supervisors manage agents in a top-down structure, similar to an organizational hierarchy, where each layer delegates tasks to the next.

While these patterns may seem obvious, choosing the right multi-agent design is critical. It ensures the system scales effectively, avoids unnecessary re-engineering, and aligns with the problem's complexity. In agentic systems, selecting the appropriate multi-agent pattern is essentially choosing the right system architecture for AI-driven workflows.

**Multi agent in hybrid system:** In a hybrid system, orchestrating multiple agents is easy. I have discussed this in my blog here: [https://medium.com/@afsal-taj06/how-golem-stands-out-in-the-world-of-ai-agents-7747c275ce94](https://medium.com/@afsal-taj06/how-golem-stands-out-in-the-world-of-ai-agents-7747c275ce94)

### Memory Management in Agents

Agents typically use both short-term memory and persistent memory together. Short-term memory holds recent context, such as in conversational agents, while persistent memory stores data indefinitely. Data stored by one agent can potentially be accessed and used by other agents.

**Memory Management in Golem:** In systems like Golem, both short-term and long-term memory are durable, making the distinction less visible. You can store relevant information in another agent — for example, a nuanced supervisor agent — that exposes tools to retrieve information from its working memory. In fact, the working memory in this supervisor agent is persistent, allowing seamless access and reuse of information across agents. This is one of the reasons I personally prefer using Golem for building agents.

### Learning and Adaptation

Learning and adaptation is a pattern specific to agentic systems. While the concept may seem obvious, it's important to understand the nuances of how agents improve over time.

- **Self-learning**: Analogous to unsupervised learning, such as clustering algorithms. Agents use this to identify patterns and adapt without explicit labels. If you are unfamiliar with how ML works, this can sound a bit overwhelming.
- **Supervised learning**: Agents use labeled data and algorithms like classification to improve decision-making. Again, this is not LLM itself, but making use of classical machine learning algorithms like SGD.
- **Few-shot learning**: LLMs quickly adapt to new situations with just a few examples, enabling rapid response to novel inputs.
- **Online learning**: Continuous learning where data from real interactions is fed back into the agent, allowing it to adapt dynamically instead of relying solely on pre-defined training datasets.

**Proximal Policy Optimization (PPO)**
In reinforcement learning, an agent's strategy for taking actions is called a policy. The agent interacts with its environment (for example, by playing a game) and collects experiences (state, action, reward). PPO computes policy updates that improve rewards while enforcing safety boundaries, often called "clipping," to prevent large, destabilizing changes to the policy.

**Direct Policy Optimization (DPO)**
Instead of relying on a reward model to fine-tune an LLM, DPO directly teaches the model which responses are preferred. It increases the probability of generating desired responses and decreases the probability of undesired ones. PPO is more probabilistic, whereas DPO is more deterministic.

Examples of agents that use learning and adaptation include AlphaEvolve, and OpenEvolve (mentioned in the book). These systems demonstrate how agents can continuously improve their performance over time through various learning techniques.

### The Model Context Protocol (MCP)

This provides a standardized interface for LLMs to interact with external resources. This protocol is a key mechanism for ensuring consistent and predictable integration across systems.

First, let's clarify a common misconception: MCP is **not** an overrated HTTP protocol. Integration with LLMs should follow a standard to avoid the need for custom, ad hoc connections. MCP operates on a client–server architecture. The server exposes tools — called MCP tools — such as a function that queries a public weather database.

With multiple LLM models in the wild, including Gemini, OpenAI's GPT models, Mixtral, and Claude, a standard way of communicating with the MCP server is essential. The host application where the LLM is used acts as the MCP client, communicating with the MCP server. This setup allows you to build an MCP tool once and use it universally, regardless of how the host application is designed or which LLM it employs.

The underlying API can also be enhanced with deterministic features such as filtering and sorting to help the non-deterministic agent work more efficiently. Agents do not magically replace deterministic workflows — they often require strong deterministic support to succeed.

### Distinction between Function Calls and MCP Tool Calls

This is an important distinction, not quite explicitly mentioned in many articles. A tool function call is a proprietary way for an LLM to invoke a specific function (or tool) directly. However, in the MCP paradigm, tool calls are handled by the host application, not the LLM itself. This distinction is often overlooked in documentation. By letting the application handle tool interactions, MCP enables **reusability of assets** without refactoring or re-engineering.

### Beyond Tools: Prompts and Resources

MCP is not just about tool interactions. It also standardizes:

- **Resources**: Static data, such as PDF files or database records.
- **Tools**: Executable functions that perform actions, like sending an email or querying an API.
- **Prompts**: Templates that guide the LLM in interacting with resources or tools, ensuring structured and effective communication.

### MCP and Golem

In Golem version 1.3, MCP support is limited. This doesn't prevent you from building agentic systems with Golem; you will need to handle some orchestration manually while adhering to MCP protocols. Version 1.4 of Golem will introduce more comprehensive MCP support, providing standardized interfaces that allow you to build agentic systems fully aligned with MCP.

## Summary

Getting excited about AI agents is natural, but we must not forget the foundational principles of robust software engineering. While I may be biased as a contributor to Golem, I firmly believe that overly intrusive frameworks — which force their patterns and functions into your code — can lead to unnecessary complexity and practical restrictions. Instead, it's wiser to leverage evolving, durable hybrid systems like Golem, which let you build AI-enhanced solutions without losing sight of established engineering practices. The key is to focus on solving problems with AI, not being consumed by AI or framework overhead.

For those who already tried out Golem, here is the 1.3 release event which for the first time, talks about agentic systems, presented by John De Goes, Daniel Vigovsky and team.

[https://www.youtube.com/watch?v=91-CH1TZG3o](https://www.youtube.com/watch?v=91-CH1TZG3o)
