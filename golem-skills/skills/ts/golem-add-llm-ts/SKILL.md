---
name: golem-add-llm-ts
description: "Adding LLM and AI capabilities to a TypeScript Golem agent. Use when the user wants to add LLM chat, embeddings, or any AI provider integration to a TypeScript agent."
---

# Adding LLM and AI Capabilities (TypeScript)

## Overview

There are no Golem-specific AI libraries for TypeScript. Instead, use **third-party npm packages** that work with the `fetch` API — Golem's TypeScript runtime provides full `fetch` support via WASI HTTP, so most LLM client libraries that use `fetch` internally will work out of the box.

## Recommended Libraries

### OpenAI

The official `openai` npm package works in Golem:

```shell
npm install openai
```

```typescript
import OpenAI from 'openai';

const client = new OpenAI({
  apiKey: process.env.OPENAI_API_KEY,
});

const response = await client.chat.completions.create({
  model: 'gpt-4o',
  messages: [{ role: 'user', content: 'Hello!' }],
});

const text = response.choices[0]?.message?.content ?? '';
```

### Anthropic

The official `@anthropic-ai/sdk` package works in Golem:

```shell
npm install @anthropic-ai/sdk
```

```typescript
import Anthropic from '@anthropic-ai/sdk';

const client = new Anthropic({
  apiKey: process.env.ANTHROPIC_API_KEY,
});

const response = await client.messages.create({
  model: 'claude-sonnet-4-20250514',
  max_tokens: 1024,
  messages: [{ role: 'user', content: 'Hello!' }],
});
```

### Other Providers

Any npm library that uses `fetch` or `node:http` internally should work. This includes:

- **Google AI** (`@google/generative-ai`) — Gemini models
- **Cohere** (`cohere-ai`) — chat, embeddings, reranking
- **Mistral** (`@mistralai/mistralai`) — Mistral models
- **Groq** (`groq-sdk`) — fast inference

### Calling Any LLM API Directly

You can also call any LLM provider's REST API directly using `fetch`:

```typescript
const response = await fetch('https://api.openai.com/v1/chat/completions', {
  method: 'POST',
  headers: {
    'Content-Type': 'application/json',
    'Authorization': `Bearer ${process.env.OPENAI_API_KEY}`,
  },
  body: JSON.stringify({
    model: 'gpt-4o',
    messages: [{ role: 'user', content: 'Hello!' }],
  }),
});

const data = await response.json();
const text = data.choices[0]?.message?.content ?? '';
```

Load the `golem-make-http-request-ts` skill for more details on making HTTP requests.

## Setting API Keys

Store provider API keys as **secrets** using Golem's typed config system. Load the `golem-add-secret-ts` skill for full details. In brief, declare the key in your config type:

```typescript
import { Config, Secret } from "@golemcloud/golem-ts-sdk";

type MyAgentConfig = {
  apiKey: Secret<string>;
};
```

Then manage it via the CLI:

```shell
golem agent-secret create apiKey --secret-type string --secret-value "sk-..."
```

Access in code with `this.config.value.apiKey.get()`.

## Complete Agent Example

```typescript
import { BaseAgent, agent, endpoint } from '@golemcloud/golem-ts-sdk';
import OpenAI from 'openai';

@agent({ mount: '/chats/{chatName}' })
class ChatAgent extends BaseAgent {
  private messages: OpenAI.ChatCompletionMessageParam[] = [];
  private client: OpenAI;

  constructor(readonly chatName: string) {
    super();
    this.client = new OpenAI({ apiKey: process.env.OPENAI_API_KEY });
    this.messages.push({
      role: 'system',
      content: `You are a helpful assistant for chat '${chatName}'`,
    });
  }

  @endpoint({ post: '/ask' })
  async ask(question: string): Promise<string> {
    this.messages.push({ role: 'user', content: question });

    const response = await this.client.chat.completions.create({
      model: process.env.LLM_MODEL ?? 'gpt-4o',
      messages: this.messages,
    });

    const reply = response.choices[0]?.message?.content ?? '';
    this.messages.push({ role: 'assistant', content: reply });
    return reply;
  }
}
```

## Key Constraints

- Use npm libraries that internally use `fetch` or `node:http` — these work in Golem's WASM runtime
- Libraries that depend on native C/C++ bindings (e.g., `onnxruntime-node`) will **not** work
- API keys should be stored as secrets using Golem's typed config system (load the `golem-add-secret-ts` skill)
- All HTTP requests made from agent code are automatically durably persisted by Golem
