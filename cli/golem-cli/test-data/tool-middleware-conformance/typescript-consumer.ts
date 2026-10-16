import { z } from 'zod';
import { defineAgent, method } from '@golemcloud/golem-ts-sdk';
import { PresentedAdapterClient } from 'compat-leaf-tool-guest-client';

const TypeScriptAdapterConformanceAgent = defineAgent({
  name: 'TypeScriptAdapterConformanceAgent',
  id: { name: z.string() },
  methods: {
    invoke: method({ input: { kept: z.string() }, returns: z.string() }),
  },
});

TypeScriptAdapterConformanceAgent.implement({
  init: () => ({}),
  methods: {
    async invoke({ kept }) {
      const output = await PresentedAdapterClient.newClient().execute({ kept });
      return `ok:${output.kept}`;
    },
  },
});
