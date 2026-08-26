import * as middlewareSdk from '@golemcloud/golem-ts-sdk/middleware';

export const middleware = middlewareSdk.universalToolMiddleware({
  name: 'middleware-only',
  invoke: (request, { underlying }) =>
    underlying.invoke(request.commandPath, request.input, request.stdin),
});

interface EmbeddedMiddlewareGuest {
  discoverToolMiddlewares(): Array<{ name: string; scope: { tag: string } }>;
  getToolMiddleware(name: string): { name: string; scope: { tag: string } };
}

const embeddedGuest = (
  middlewareSdk as unknown as {
    golemTool010ToolMiddlewareGuest: EmbeddedMiddlewareGuest;
  }
).golemTool010ToolMiddlewareGuest;

if (!embeddedGuest) {
  throw new Error('selected wrapper does not expose the middleware guest runtime');
}

const discovered = embeddedGuest.discoverToolMiddlewares();
if (
  discovered.length !== 1 ||
  discovered[0]?.name !== 'middleware-only' ||
  discovered[0]?.scope.tag !== 'universal'
) {
  throw new Error('selected wrapper did not discover the registered middleware');
}

const selected = embeddedGuest.getToolMiddleware('middleware-only');
if (selected.name !== 'middleware-only' || selected.scope.tag !== 'universal') {
  throw new Error('selected wrapper did not retrieve the registered middleware');
}
