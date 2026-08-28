import { ok, toolDefinition } from '@golemcloud/golem-ts-sdk';

const ordinaryToolDefinition = toolDefinition('ordinary').command('ping', (ping) =>
  ping.body((body) => body),
);

export const ordinaryTool = ordinaryToolDefinition.implement({
  ping: async () => ok(undefined),
});
