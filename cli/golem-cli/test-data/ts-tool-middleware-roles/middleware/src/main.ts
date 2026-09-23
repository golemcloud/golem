import { toolMiddlewareGuest } from "@golemcloud/golem-ts-sdk";
import { universalToolMiddleware } from "@golemcloud/golem-ts-sdk/middleware";

export const middleware = universalToolMiddleware({
  name: "middleware-only",
  invoke: (request, { underlying }) =>
    underlying.invokeAndAwait(
      request.commandPath,
      request.input,
      request.stdin,
    ),
});

interface EmbeddedMiddlewareGuest {
  discoverToolMiddlewares(): Array<{ name: string; scope: { tag: string } }>;
  getToolMiddleware(name: string): { name: string; scope: { tag: string } };
}

const embeddedGuest = toolMiddlewareGuest as EmbeddedMiddlewareGuest;

if (!embeddedGuest) {
  throw new Error(
    "selected wrapper does not expose the middleware guest runtime",
  );
}

const discovered = embeddedGuest.discoverToolMiddlewares();
if (
  discovered.length !== 1 ||
  discovered[0]?.name !== "middleware-only" ||
  discovered[0]?.scope.tag !== "universal"
) {
  throw new Error(
    "selected wrapper did not discover the registered middleware",
  );
}

const selected = embeddedGuest.getToolMiddleware("middleware-only");
if (selected.name !== "middleware-only" || selected.scope.tag !== "universal") {
  throw new Error(
    "selected wrapper did not retrieve the registered middleware",
  );
}
