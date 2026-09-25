import { Effect, FileSystem, Layer, Path, Ref, Schema, Stream } from "effect";
import { Etag, HttpPlatform, HttpRouter as Routes, HttpServerRequest, HttpServerResponse } from "effect/unstable/http";
import * as HttpEffect from "effect/unstable/http/HttpEffect";
import { HttpApi, HttpApiBuilder, HttpApiEndpoint, HttpApiGroup, OpenApi } from "effect/unstable/httpapi";
import { defineAgent, method, Http, HttpRouter as RootHttpRouter } from "@golemcloud/effect-golem";
import * as HttpRouter from "@golemcloud/effect-golem/HttpRouter";

if (HttpRouter.define !== RootHttpRouter.define || HttpRouter.withRawHeaders !== RootHttpRouter.withRawHeaders) {
  throw new Error("SDK subpath imports must share the embedded router and registry");
}

const Counter = defineAgent({
  name: "RouterCounter", id: { name: Schema.String },
  methods: { increment: method({ input: {}, success: Schema.Int }) },
});
Counter.implement({
  init: () => Ref.make(0),
  methods: (count) => ({ increment: () => Ref.updateAndGet(count, (n) => n + 1) }),
});

const routes = Layer.mergeAll(
  Routes.add("POST", "/echo", Effect.gen(function* () {
    const request = yield* HttpServerRequest.HttpServerRequest;
    return HttpRouter.withRawHeaders(HttpServerResponse.stream(request.stream), [
      { name: "set-cookie", value: new TextEncoder().encode("a=1; Expires=Wed, 21 Oct 2030 07:28:00 GMT") },
      { name: "x-public", value: new TextEncoder().encode("between") },
      { name: "set-cookie", value: new TextEncoder().encode("b=2; Path=/") },
    ]);
  })),
  Routes.add("GET", "/head", HttpServerResponse.stream(Stream.suspend(() => {
    throw new Error("HEAD body started");
  }), { headers: { "content-length": "99" } })),
  Routes.add("POST", "/early", HttpServerResponse.text("early", { status: 202 })),
  Routes.add("GET", "/counter", Effect.gen(function* () {
    const counter = yield* Counter.client.get({ name: "http" });
    return HttpServerResponse.text(String(yield* counter.increment({})));
  })),
  Routes.add("GET", "/before-head", Effect.die(new Error("before-head failure"))),
  Routes.add("GET", "/after-head", HttpServerResponse.stream(Stream.concat(
    Stream.succeed(new TextEncoder().encode("prefix")),
    Stream.fromEffect(Effect.sleep("100 millis").pipe(Effect.andThen(Effect.fail(new Error("after-head failure"))))),
  ))),
  Routes.add("GET", "/hook", HttpEffect.withPreResponseHandler(
    Effect.succeed(HttpServerResponse.text("hook")),
    (_, response) => Effect.succeed(HttpServerResponse.setHeader(response, "x-hook", "shared")),
  )),
);
HttpRouter.define("EffectWeb", { mount: Http.mount("/web") }).implement(Routes.toHttpEffect(routes));

HttpRouter.define("EffectRaw", { mount: Http.mount("/raw") }).implement(Effect.succeed(Effect.gen(function* () {
  const request = yield* HttpRouter.request;
  return HttpServerResponse.jsonUnsafe({ method: request.method, path: request.path, query: request.query ?? null });
})));

HttpRouter.define("EffectStatic", {
  mount: Http.mount("/static"), static: [{ route: "/*", path: "/assets/$1" }],
}).register();

const Api = HttpApi.make("Catalog").add(HttpApiGroup.make("items").add(
  HttpApiEndpoint.get("getItem", "/items/:id", {
    params: Schema.Struct({ id: Schema.String }), success: Schema.Struct({ id: Schema.String, count: Schema.Int }),
  }),
));
const Handlers = HttpApiBuilder.group(Api, "items", (handlers) => handlers.handle("getItem", ({ params }) => Effect.succeed({ id: params.id, count: 7 })));
const Platform = Layer.mergeAll(Path.layer, Etag.layerWeak, FileSystem.layerNoop({}), HttpPlatform.layer.pipe(Layer.provide(FileSystem.layerNoop({}))));
HttpRouter.define("EffectCatalog", {
  mount: Http.mount("/catalog"), openApi: Effect.sync(() => OpenApi.fromApi(Api)),
}).implement(Routes.toHttpEffect(HttpApiBuilder.layer(Api).pipe(Layer.provide(Handlers), Layer.provide(Platform))));
