import { z } from 'zod';
import { defineAgent, method, clientFor } from '@golemcloud/golem-ts-sdk';

export const StringAgent = defineAgent({
  name: 'StringAgent',
  id: { name: z.string() },
  methods: {
    test: method({ input: {}, returns: z.void() }),
  },
});

export const StringAgentImpl = StringAgent.implement({
  init: () => ({}),
  methods: {
    test() {},
  },
});

const StructArgs = z.object({ x: z.string(), y: z.string(), z: z.string() });

export const StructAgent = defineAgent({
  name: 'StructAgent',
  id: { args: StructArgs },
  methods: {
    test: method({ input: {}, returns: z.void() }),
  },
});

export const StructAgentImpl = StructAgent.implement({
  init: () => ({}),
  methods: {
    test() {},
  },
});

const stringClient = clientFor(StringAgent);
const structClient = clientFor(StructAgent);

async function runStringTest(): Promise<void> {
  for (let i = 445; i < 450; i++) {
    await stringClient({ name: ' '.repeat(i) }).test();
  }
}

async function runStructTest(): Promise<void> {
  for (let i = 100; i < 105; i++) {
    await structClient({
      args: { x: ' '.repeat(i), y: ' '.repeat(i), z: '/'.repeat(i) },
    }).test();
  }
}

export const TestAgent = defineAgent({
  name: 'TestAgent',
  id: { name: z.string() },
  methods: {
    testAll: method({ input: {}, returns: z.void() }),
    testString: method({ input: {}, returns: z.void() }),
    testStruct: method({ input: {}, returns: z.void() }),
  },
});

export const TestAgentImpl = TestAgent.implement({
  init: () => ({}),
  methods: {
    async testAll() {
      await runStringTest();
      await runStructTest();
    },
    async testString() {
      await runStringTest();
    },
    async testStruct() {
      await runStructTest();
    },
  },
});
