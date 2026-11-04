import { Effect, Schema } from "effect";
import { FetchHttpClient, HttpClient, HttpClientRequest, HttpClientResponse } from "effect/http";
import { defineAgent, method } from "@golemcloud/effect-golem";

export function registerProbe(withTimeout: boolean) {
  defineAgent({
    name: "HttpUpdateProbe",
    mode: "durable",
    id: { name: Schema.String },
    methods: {
      run: method({ input: { url: Schema.String }, success: Schema.String }),
      ping: method({ input: {}, success: Schema.String }),
    },
  }).implement({
    init: () => Effect.void,
    methods: () => ({
      run: ({ url }) => Effect.gen(function* () {
        const request = HttpClientRequest.post(url).pipe(
          HttpClientRequest.bodyText("hello"),
          HttpClient.execute,
          Effect.flatMap(HttpClientResponse.filterStatusOk),
        );
        const response = yield* (withTimeout ? request.pipe(Effect.timeout("10 seconds")) : request);
        return yield* response.text;
      }).pipe(Effect.provide(FetchHttpClient.layer), Effect.orDie),
      ping: () => Effect.succeed(withTimeout ? "timeout" : "baseline"),
    }),
  });
}
