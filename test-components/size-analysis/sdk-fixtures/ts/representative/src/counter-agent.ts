import { z } from 'zod';
import { defineAgent, method, http } from '@golemcloud/golem-ts-sdk';
import { fromByteArray } from 'base64-js';
import { TsStreamingClient } from 'ts-streaming-tool-guest-client';

const payload = '__SIZE_PAYLOAD__';

export const SizeHelper = defineAgent({
  name: 'SizeHelper',
  id: { name: z.string() },
  methods: {
    value: method({ input: {}, returns: z.number() }),
    add: method({ input: { by: z.number() }, returns: z.number() }),
  },
});

SizeHelper.implement({
  init: () => ({ count: 0 }),
  methods: {
    value() { return this.count; },
    add({ by }) { this.count += by; return this.count; },
  },
});

export const CounterAgent = defineAgent({
  name: 'CounterAgent',
  id: { name: z.string() },
  config: { label: z.string() },
  http: http.mount('/counters/{name}'),
  methods: {
    value: method({ input: {}, returns: z.number(), http: http.get('/value') }),
    add: method({ input: { by: z.number() }, returns: z.number() }),
    configured: method({ input: {}, returns: z.string() }),
    payloadLength: method({ input: {}, returns: z.number() }),
    slice: method({ input: { start: z.number(), length: z.number() }, returns: z.string() }),
    dependency: method({ input: { start: z.number(), length: z.number() }, returns: z.string() }),
    rpcAdd: method({ input: { by: z.number() }, returns: z.number() }),
    tool: method({ input: {}, returns: z.string() }),
  },
});

CounterAgent.implement({
  init: ({ id }) => ({ name: id.name, count: 0 }),
  methods: {
    value() { return this.count; },
    add({ by }) { this.count += by; return this.count; },
    configured() { return this.config.label; },
    payloadLength() { return payload.length; },
    slice({ start, length }) { return payload.slice(start, start + length); },
    dependency({ start, length }) {
      return fromByteArray(new TextEncoder().encode(payload.slice(start, start + length)));
    },
    async rpcAdd({ by }) {
      return (await SizeHelper.client.upsert({ name: this.name })).add({ by });
    },
    async tool() {
      const input = new ReadableStream<Uint8Array>({
        start(controller) { controller.enqueue(new Uint8Array([65, 0, 255, 90])); controller.close(); },
      });
      const invocation = TsStreamingClient.newClient().ts_streaming('echo', input);
      if (!invocation.stdout) throw new Error('missing tool stdout');
      const consume = async () => {
        const reader = invocation.stdout!.getReader();
        const bytes: number[] = [];
        try {
          for (;;) {
            const chunk = await reader.read();
            if (chunk.done) return bytes;
            bytes.push(...chunk.value);
          }
        } finally { reader.releaseLock(); }
      };
      const [count, bytes] = await Promise.all([invocation.result, consume()]);
      return `${count}:${fromByteArray(new Uint8Array(bytes))}`;
    },
  },
});
