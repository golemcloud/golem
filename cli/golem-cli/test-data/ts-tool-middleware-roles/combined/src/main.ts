import { ok, toolDefinition, universalToolMiddleware } from '@golemcloud/golem-ts-sdk';

const combinedToolDefinition = toolDefinition('combined').command('ping', (ping) =>
  ping.body((body) => body),
);

export const combinedTool = combinedToolDefinition.implement({
  ping: async () => ok(undefined),
});

export const middleware = universalToolMiddleware({
  name: 'combined-middleware',
  invoke: (request, { underlying }) =>
    underlying.invoke(request.commandPath, request.input, request.stdin),
});
