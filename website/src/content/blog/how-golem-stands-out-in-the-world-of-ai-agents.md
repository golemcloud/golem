---
title: "How does Golem stand out in the world of AI Agents?"
date: "2025-10-13"
author: "Afsal Thaj"
tags: ["Industry Articles", "AI Agent"]
slug: "how-golem-stands-out-in-the-world-of-ai-agents"
originalUrl: "https://golem.cloud/post/how-golem-stands-out-in-the-world-of-ai-agents"
---

## 🔹 AI Agent

An AI agent is a system powered by an LLM (or other AI models) that can:

1. Get an input from the user (e.g. “book me a flight”).
2. Decide what to do next: Example: The next best thing to do at this stage is to call a weather-api
3. Act by calling tools, APIs, or other systems.
4. To a reasonable extent, keeping the memory of what’s going on, so that it knows the context
5. Adapt to changes automatically (to a great extent)

It’s not feasible to explain every fundamental of an agentic system in this blog. So for those who are new to this, it’s good to have a reasonable idea about various concepts lying around an agentic system such as LLM models, tools etc before you deep dive into the core subject of this blog on how Golem supersedes many of the existing agentic frameworks.

## 🔹 AI being perceived as an escape route from system design.

We love automation, but we hate losing control. A solid system design allows a business to stay in control while still harnessing the power of AI.

Being in the AI space is not an excuse to stop strengthening our understanding of system design fundamentals. The challenges you solve through classic system design remain just as relevant, even when you move into the agentic space. It’s definitely not an escape route for those who are starting to build a career in software engineering with AI.

## 🔹 Polluting the idea of AI agents with class system design problems

This may be a bit opinionated, but you’re welcome to consider my other thoughts shared publicly before reflecting on the points below.

Wherever I search for AI agents, the answer is not just AI agents, but AI agents along with something else. I call it “polluting the idea of AI agents” — as if somebody deliberately pulling the whole thing back to square one.

The main questions are as follows:

- When I use this framework, did you expose a DSL such that you (your runtime) know how I interacted with an LLM model (GPT4 as an example) in a certain way?
- Which part of your example is really an AI agent? Is everything an AI agent? Where is that boundary between agents and non agents within your system?
- Why does my code look different compared to what I would have normally written while still using the reasoning and decision making power of AI to design the workflow? End of the day, everything is still a code.

These doubts don’t question the protocols in place (such as MCP) to make the design of an agentic system well structured. But it questions how frameworks generally go about solving these problems.

As you read more below, you will end up seeing these opinions with more clarity and becomes more of a problem statement.

## 🔹 Current AI agent frameworks

Before I clarify the problem with better examples in code, let’s be aware of the existing frameworks that help with building things around AI agents and it’s workflow.

- LangGraph/LangChain/Open SWE
- Microsoft Agent Framework
- AutoGen
- Semantic Kernel
- Dapr Agents
- SuperAGI

There might be a few others. I searched AI to get a couple of these names :)

These are listed here for sake of completion of this blog.

It’s impossible for me to write down the comparison of each of these with Golem in this blog. But I will be continuing with my comparisons, and will hopefully write them down if its worth it.

_In this blog, I used LangGraph as only a reference to explaining the fundamental reason as to why Golem stands out in the world of agentic, and why it is important for developers to have an eye on the entire ecosystem of Golem._

## 🔹 Let’s build an agent using LangGraph

[LangGraph](https://www.langchain.com/langgraph) is built on top of LangChain, that utilises concepts of Graph such as nodes, edges, to represent functions, execution and data flow along with an inbuilt state machine. Specifically, it enables cyclic topologies.

Hopefully the below example (copied from various blogs and documentations within LangGraph) shows what exactly this is.

### An example of invoking LLM

Here is a code snippet that makes use of an LLM model with necessary tools bound to the LLM model.

```python
llm = ChatOpenAI(model="gpt-4o-mini", api_key="sk-U7tijaa4jwHvhVWGr....", temperature=0)

# where search_web and get_weather are functions decorated with @tools
tools = [search_web, get_weather] 
query = "What is the weather in NYC?"
response = llm.invoke(query)
print(response.content)
```

Under the hood, LLM decides which tools to invoke and responds with a tool response (the tools that you should call based on the prompt), and the local orchestration of LangGraph ensures to call that tool function to get a possible non-user friendly response, which it passes to LLM again to get a reasonably human friendly summary or response. Example: It peels off the lat-long details, or exact temperature in degrees etc.

```
The weather in NYC is sunny
```

Note that it was not the remote LLM runtime that calls the function get-weather, but it’s done at the client side.

### A Claude Desktop way of doing it (to get better understanding)

If you need an outside view of a similar workflow using Claude Desktop, [https://modelcontextprotocol.io/docs/develop/build-client#python](https://modelcontextprotocol.io/docs/develop/build-client#python) is a good starting point where it tries to achieve a similar outcome, while exposing the idea of MCP protocol in a more tangible way.

### An example of a pre-built agent

This example is copied from this [blog](https://medium.com/pythoneers/building-ai-agent-systems-with-langgraph-9d85537a6326#4037) that explains LangGraph without getting lost in too many details.

```python
from langgraph.prebuilt import create_react_agent

# system prompt is used to inform the tools available to when to use each
system_prompt = """Act as a helpful assistant.
    Use the tools at your disposal to perform tasks as needed.
        - get_weather: whenever user asks get the weather of a place.
        - search_web: whenever user asks for information on current events or if you don't know the answer.
    Use the tools only if you don't know the answer.
    """
agent = create_react_agent(model=llm, tools=tools, state_modifier=system_prompt)
def print_stream(stream):
    for s in stream:
        message = s["messages"][-1]
        if isinstance(message, tuple):
            print(message)
        else:
            message.pretty_print()
inputs = {"messages": [("user", "What is the current weather in Budapest today")]}
print_stream(agent.stream(inputs, stream_mode="values"))
```

We will see why this was called a pre-built agent, once we see the details of a custom agent, which gives us the handle to the state machine and control things ourselves.

### An example of a custom-built

```python
tools = [search_web, get_weather]
tool_node = ToolNode(tools)

def call_model(state: MessagesState):
    messages = state["messages"]
    response = llm_with_tools.invoke(messages)
    return {"messages": [response]}
def call_tools(state: MessagesState) -> Literal["tools", END]:
    messages = state["messages"]
    last_message = messages[-1]
    if last_message.tool_calls:
        return "tools"
    return END

# initialize the workflow from StateGraph
workflow = StateGraph(MessagesState)
# add a node named LLM, with call_model function. This node uses an LLM to make decisions based on the input given
workflow.add_node("LLM", call_model)
# Our workflow starts with the LLM node
workflow.add_edge(START, "LLM")
# Add a tools node
workflow.add_node("tools", tool_node)
# Add a conditional edge from LLM to call_tools function. It can go tools node or end depending on the output of the LLM. 
workflow.add_conditional_edges("LLM", call_tools)
# tools node sends the information back to the LLM
workflow.add_edge("tools", "LLM")
agent = workflow.compile()

for chunk in agent.stream(
    {"messages": [("user", "Will it rain in Budapest today?")]},
    stream_mode="values",):
    chunk["messages"][-1].pretty_print()
```

This is a typical example of a custom agent built using LangGraph that involves a state **_`MessageState`_** which holds the state of computation defined at every `node`, with the next best step defined as graph **_`edge`_ .**

First LLM is invoked, and based on the tools available, it suggests a tool that the backend of LangGraph can invoke, and then based on the outcome, it can either go into actual invocation of tool (which is something you told LangGraph) depending on the state of computation in **_`MessageState` ._**

## 🔹 That’s where the agentic aspect starts to get diluted

While there is an LLM call in the process — the key part of the example above, in the context of this discussion, lies in its **custom state management**. This mechanism only comes into play when we use **nodes** (to represent functions) and **graph edges** (to define the workflow blueprint).

This is exactly where the line between **true agentic behavior** and **standard stateful workflow management** begins to blur. If we continue building on the same example, this distinction becomes even harder to see. So, let’s take the same LangGraph API and attempt to construct the **simplest possible version**, so we can observe where the boundaries truly lie.

## 🔹 Let’s build the most simplest Agent to get things clearer

In a complex workflow, we might have dozens of agents interacting with one another. Instead of diving into the specifics of how they communicate, what each agent is responsible for, or which tools they use internally, let’s focus on a single agent — one that’s **intentionally stripped of LLM details**.

Yes, it’s still an agent — just in its most minimal, essential form.

## 🔹 A counter agent in LangGraph

As mentioned before, we should be comfortable by now to strip off the LLM details and build a custom agent that is simply a counter agent. i.e, keep calling an `increment` and it simply increments the previous number by 1.

```python
from langgraph.graph import StateGraph, END

# Define the agent's state shape
class CounterState(dict):
    count: int
    name: str
# Define nodes (functions that update state)
def increment_node(state: CounterState):
    new_value = state.get("count", 0) + 1
    return {"count": new_value}
# Build the graph
graph = StateGraph(CounterState)
# Add nodes
graph.add_node("increment", increment_node)
# Define transitions
graph.add_edge("increment", END)
# Compile into a runnable workflow
counter_agent = graph.compile()
# Run the agent
state = {"count": 0, "name": "myCounter"}
result = counter_agent.invoke(state, {"action": "increment"})
print(result)  # {"count": 1, "name": "myCounter"}
```

### LangGraph Persistance

The CounterState can be made reliable using various plugins or other features using LangGraph. Example: A persistence plugin can allow you to make sure the state is never lost!

```python
checkpointer = InMemorySaver()
counter_agent = graph.compile(checkpointer=checkpointer)
```

This is a code snippet copied from checkpoints section from LangGraph [documentation](https://langchain-ai.github.io/langgraph/concepts/persistence/#checkpoints).

Yes, we did consider a simple counter as an agent, which may or may not become part of a complex graph within LangGraph. Given we considered Counter as an “Agent”, we ended up in a non [ubiquitous](https://martinfowler.com/bliki/UbiquitousLanguage.html) code that is concerned with other irrelevant aspects — just to write a super simple logic.

## 🔹 A counter agent in Golem

Welcome to [Golem](https://www.golem.cloud/)! Let’s get straight to the hands-on instead of going around with details, by implementing the “Counter” using Golem.

```typescript
import { BaseAgent, agent } from '@golemcloud/golem-ts-sdk';

@agent()
class CounterAgent extends BaseAgent {
    private value: number = 0;
    async increment(): Promise<number> {
        this.value += 1;
        return this.value;
    }
}
```

## 🔹 It’s a normal typescript code in Golem.

It’s a counter **_agent_** with its state durable (meaning reliable that it won’t lose its state during a system crash as an example) by default. Everything else other than being agentic due to the use of `@agent` is a normal typescript code that you would otherwise be writing in implementing a production grade counter.

### Boundary between Agent and Non-Agent is clear

To explain this aspect, let’s stop considering `Counter` to be agent. To do this, all you need to do is remove the decorator `@agent` from it. Later you can consider it to be a **_tool_** which your other agents can internally use.

**_Had this been a simple tool instead, the state of the counter is still reliable in Golem with zero effort from developer._**

```typescript
class Counter {
    private value: number = 0;

    async increment(): Promise<number> {
        this.value += 1;
        return this.value;
    }
}
```

On the flip side, think about making your **_`CounterAgent`_** which you wrote using **_`LangGraph`_** to be non agentic and convert it to a tool. This would result in almost a full rewrite.

This is the proof that [agent native runtime](https://www.golem.cloud/) allows better orthogonality in your design.

## 🔹 Agent to Agent and scalability by default

This is the next core system design problem that Golem automatically handles without polluting your Agentic code.

Here is the problem: How to make agents run and scale independently while talking to each other ?

The answer is in code:

```typescript
@agent()
class AssistantAgent extends BaseAgent {
  constructor(readonly username: string) {
    super()
  }
  async query(input: string): Promise<string> {
    const remoteWeatherAgent = WeatherAgent.get(this.username);
    
    // Use LLM to get the actual location argument from the input
    const weather = await remoteWeatherAgent.getWeather("NYC")
    
    return `Hello ${this.username}, you asked about "${input}". Here's the weather info: ${weather}`;
  }
}

@agent()
class WeatherAgent extends BaseAgent {
  constructor(private readonly username: string) {
    super();
  }
  async getWeather(input: string): Promise<string> {
    // may be internally use LLM models to get the weather
    return "It's sunny and 25C"
  }
}
```

Given you considered **_`AssistantAgent`_** and **_`WeatherAgent`_** to be agents using `@agent` decorator, golem automatically runs them as separate tasks (or process) which may or may not run in a single `node` (or computer). With **_`WeatherAgent.`_**`get` you summoned a client which allows you to talk to a remote weather agent. This is possible with zero intervention into infrastructure!

With zero development effort, every user (defined by username above) will have their own weather agent and assistant agent running. Depending on the constructor parameters of your agent class, you can further control this scaling. They can independently scale up or down with zero memory usage when they are idle. Had there been a state in any of these agents, that is also kept intact. The state can be kept globally too.

Most often, an agent does only 1 thing, and they need to independently scale (micro services), and this is achieved by default in golem. In a way, we can call it micro service by default, but without the hassle of managing a micro service oriented design.

Along with scalability, there are other aspects of system designs which you are happy to forget when writing agents in **_`Golem`_**` `to a very significant extent such as double fire issues (exactly-once semantics).

## 🔹Why Golem is devoid of other problems in your agent?

Golem is an ecosystem with a **_runtime_** natively talking about agents while solving hard problems of system design automatically, and not just a **_library or SDK_**. The fundamental lies in leveraging static analysis of the code you write to deliver **automated reliability**. This is not done by most of the other frameworks, where you as a developer need to let the local backend (the SDK implementation) know about every step you jump through in your code. This is why, we saw graphs and nodes in the case of LangGraph while implementing a counter.

On the other hand, in golem, this static observation of the code is fed into its **runtime and rest is handled by it. It has all the information of what you wrote. Example: It knows where exactly an IO happens to call another agent.**This is the reason it is devoid of an explicit state management or persistence or scaling.

Thus, it allows developers to just focus on business logic!

## 🔹 Encoding workflow in a graph vs simple typescript instructions

In LangGraph, you encode what you need in a graph (or use a function that internally creates a graph at runtime) as nodes and edges. Every function will become a node, with transitions being encoded as edges.

Back in Golem, this is a normal set of instructions that you usually write in typescript. Call first agent, and then call the next, and then call the third one, and stop if needed or loop through. There is no special things to exercise to get it going.

## 🔹 And Why LangGraph code feels more complicated?

**It’s a _library_**, not a **_runtime as such_**. This is mostly in a philosophical sense, as you shouldn’t be confused with LangGraph’s special runtime: [https://langchain-ai.github.io/langgraph/concepts/pregel/](https://langchain-ai.github.io/langgraph/concepts/pregel/).

If runtime is devoid of the important details of the code by default, this indirectly you need to help it out with complex APIs even to do a simple function call, with a mixed bag of plugins that could solve various classic system design problems while busy dealing with the aspects of LLM and tool calls, and others. In other words, it offloads to developers to manage the boundary between what matters as an agent and what not.

In other words, it relies on the aspect of somehow informing the backend about everything that you do: the agent’s state, its decisions and transitions. That’s why you saw me write a **_`StateGraph`_**, a `node`, `edges`, and `END`. The best possible future for these type of frameworks is simplifying this information transmission and not getting rid of the need of having to pass this information.

**Explicit state passing**. In Golem, you just write `this.value += 1`, and the runtime makes `value` durable. In LangGraph, you pass around a **_`state`_** dictionary that you mutate step by step as part of a graph which it would later analyse. Mainly, **Reliability is “bring your own”**. By default, if you restart the process, your counter resets to `0`. You must add a persistence layer (checkpoints, databases) if you want durability.

In short it’s still a **framework, not infrastructure**. This is not an issue of LangGraph by itself. In fact, I consider LangGraph to be a matured, well focussed framework and does its job very well for what it is designed to. For those who want to get a gist of AI agents as a hands-on, LangGraph (and its ecosystem including Open SWE) is always on the top of various other frameworks.

Good documentations along with well defined APIs are two strong points of LangGraph and I am sure it is going to evolve very quickly with its presence in open source.

## 🔹Golem still claims SDK independence?

For those who were already familiar with Golem’s durability, you might be familiar with the claim of Golem being SDK independent. In other words, “you write code normally as you do, and let Golem take care of the rest”.

I believe the above examples clearly show how much SDK independent Golem is compared to a framework that strongly depends on its library functions to get things going. SDKs do exist to make things easier for developers and not exactly for golem’s solution to durability and exactly once semantics to work.

More recently, Golem has made a natural leap into the **agentic space**, with its **runtime natively understanding the semantics of agents** — with a strong focus on **developer experience (DX). So what we get is an ecosystem that includes a runtime and not just a set of SDK wrapped over the other.**

## 🔹 Agent frameworks are batteries included for AI. How about Golem?

Here is an example that gives an idea of what Golem can offer. It is mostly self explanatory.

```typescript
import * as llm from 'golem:llm/llm@1.0.0';
import * as webSearch from 'golem:web-search/web-search@1.0.0';

@prompt("What topic do you want to research?")
  async research(topic: string): Promise<string> {
    const searchResult = searchWebForTopic(topic)
    let llmResult = llm.send(
      [
        {
          tag: "message",
          val: {
            role: "assistant",
            name: "research-agent",
            content: [
              {
                tag: "text",
                val: `
                  I'm writing a report on the topic "${ topic }",
                  Your job is to be a research-assistant and provide me an initial overview on the topic so I can dive into it in more detail.
                  At the bottom are top search results from a search engine in json format. Prioritize objective and reliable sources.
                  Search results: ${ JSON.stringify(searchResult) }
                `
              }
            ]
          }
        }
      ],
      {
        model: this.model,
        tools: [],
        providerOptions: []
      }
    );
    const textResult = llmResult.content.filter(content => content.tag === "text").map(content => content.val).join("\n");
    return `Finished research for topic ${ topic }:\n${ textResult }`
  }

function searchWebForTopic(topic: string): SearchResult[] {
  const pagesToRetrieve = 3
  const session = webSearch.startSearch({
    query: topic,
    language: "lang_en",
    safeSearch: "off",
    maxResults: 10,
    advancedAnswer: true
  })
  const content: SearchResult[] = []
  for (let i = 0; i < pagesToRetrieve; i++) {
    const page = session.nextPage()
    for (let item of page) {
      content.push({
        url: item.url,
        title: item.title,
        snippet: item.snippet
      })
    }
  }
  return content
}
```

Golem has inhouse libraries that you can depend on to talk to any LLM providers such as the following:

- Anthropic
- Ollama
- OpenAI
- Amazon Bedrock
- OpenRouter
- Grok

It also includes search providers such as the following:

- Algolia
- Elasticsearch
- Meilisearch
- Opensearch
- Typesense

Also, dependencies for Video Generation Providers, Speech-Text providers, and Websearch providers are also available.

In fact you can build your own framework that makes use of the above integrations.

## 🔹 Future: MCP protocol support and other integrations

The **agentic ecosystem** in Golem is still **brand new**, but it’s evolving rapidly — with frequent, feature-rich releases that will make it increasingly reliable and “batteries-included.”

As of this writing, Golem is preparing its **final 1.3 release**, after which versions **1.3.1 through 1.4** will focus on making the platform more accessible and powerful for **advanced agent developers**.

Key priorities include deep integration with the MCP protocol, a well-defined workflow with tools, resources, and prompts, and code-first endpoints that allow developers to expose tools and other Golem functions as HTTP endpoints.

These additions will unlock new possibilities — such as **interoperability with clients like Claude Desktop** and other MCP-compatible environments.

Hope you enjoyed this writing!
