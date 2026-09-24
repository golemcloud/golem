import { AgentStream, defineAgent, method, s } from '@golemcloud/golem-ts-sdk';
import { z } from 'zod';

const item = z.object({ count: s.u32(), labels: z.array(z.string().optional()) });
const counter = defineAgent({
  name: 'Counter',
  id: {},
  config: { group: z.object({ title: z.string() }).optional(), key: s.secret(z.string()) },
  methods: {
    scalar: method({ input: { value: s.u32() }, returns: s.u32() }),
    remote: method({ input: { value: s.u32() }, returns: s.u32() }),
    stream: method({
      input: { values: s.stream(item), suffix: z.string() },
      returns: s.stream(item),
    }),
    configured: method({ input: {}, returns: z.string() }),
  },
});

counter.implement({
  init: () => ({}),
  methods: {
    scalar: ({ value }) => value + 1,
    remote: ({ value }) => counter.client.get({}).scalar({ value }),
    configured() {
      return `${this.config.group?.title ?? 'absent'}:${this.config.key.get()}`;
    },
    stream({ values, suffix }) {
      return AgentStream.from(
        (async function* () {
          for await (const value of values) {
            yield { count: value.count + 3, labels: [...value.labels, suffix] };
          }
        })(),
      );
    },
  },
});
