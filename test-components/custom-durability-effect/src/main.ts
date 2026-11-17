import { Deferred, Effect, Schema, SchemaGetter } from "effect";
import { defineAgent, Durability, method } from "@golemcloud/effect-golem";

const Port = { port: Schema.Number };
const Results = Schema.Array(Schema.String);

const NoInput = Schema.Struct({});
const Generated = Schema.Struct({ result: Schema.String });
const GeneratorFailure = Schema.Struct({ code: Schema.String });

const url = (port: number, payload: string) =>
  `http://localhost:${port}/callback?payload=${payload}`;

// The continuation after `fetch` resolves runs as a native Promise job, outside any fiber step.
// Interruption aborts the request, so an interrupted body leaves no pending host call.
const get = (port: number, payload: string): Effect.Effect<string> =>
  Effect.tryPromise(async (signal) => {
    const response = await fetch(url(port, payload), { signal });
    return await response.text();
  }).pipe(Effect.orDie);

const getAfterTimer = (port: number, payload: string): Effect.Effect<string> =>
  Effect.tryPromise(async (signal) => {
    await new Promise<void>((resolve) => setTimeout(resolve, 1));
    await new Promise<void>((resolve) => queueMicrotask(resolve));
    const response = await fetch(url(port, payload), { signal });
    return await response.text();
  }).pipe(Effect.orDie);

const durable = <R>(fn: string, body: Effect.Effect<string, never, R>) =>
  Durability.wrapInfallible(
    {
      iface: "golem-it",
      function: fn,
      functionType: Durability.FunctionType.writeRemote,
      requestSchema: NoInput,
      success: Generated,
    },
    {},
    body.pipe(Effect.map((result) => ({ result }))),
  ).pipe(Effect.map(({ result }) => result));

const generator = (port: number, payload: string) =>
  durable("test-generator", get(port, payload));

const parent = <R>(body: Effect.Effect<string, never, R>) =>
  durable("test-generator-parent", body);

const unbounded = { concurrency: "unbounded" } as const;

// Sends `<payload>-done` once the operation has ended, so the server can hold a sibling until then.
const thenSignal =
  (port: number, payload: string) =>
  <A, E, R>(operation: Effect.Effect<A, E, R>) =>
    Effect.tap(operation, () => get(port, `${payload}-done`));

const Spec = defineAgent({
  name: "EffectCustomDurability",
  id: { name: Schema.String },
  methods: {
    warmup: method({ input: {}, success: Schema.Void }),
    callback: method({ input: { ...Port, payload: Schema.String }, success: Schema.String }),
    nestedNoInput: method({ input: Port, success: Results }),
    concurrentNoInput: method({ input: Port, success: Results }),
    concurrentTimerBodies: method({ input: Port, success: Results }),
    concurrentNested: method({ input: Port, success: Results }),
    outsideScopeCall: method({ input: Port, success: Results }),
    encoderCall: method({ input: Port, success: Results }),
    concurrentFirstFinishes: method({ input: Port, success: Results }),
    failureAndInterruption: method({ input: Port, success: Results }),
    escapedWork: method({ input: Port, success: Results }),
    survivingChild: method({ input: Port, success: Results }),
  },
});

Spec.implement({
  init: () => Effect.void,
  methods: () => ({
    warmup: () => Effect.void,

    callback: ({ port, payload }) => generator(port, payload),

    nestedNoInput: ({ port }) =>
      Effect.gen(function* () {
        const outer = yield* parent(generator(port, "nested"));
        const rootSibling = yield* generator(port, "root-sibling");
        return [outer, rootSibling];
      }),

    // Identical names and requests; the server answers "slow" only after "fast" has ended.
    concurrentNoInput: ({ port }) =>
      Effect.all(
        [generator(port, "slow"), generator(port, "fast").pipe(thenSignal(port, "fast"))],
        unbounded,
      ),

    concurrentTimerBodies: ({ port }) =>
      Effect.all(
        [
          durable("test-generator", getAfterTimer(port, "timer-slow")),
          durable("test-generator", getAfterTimer(port, "timer-fast")).pipe(
            thenSignal(port, "timer-fast"),
          ),
        ],
        unbounded,
      ),

    concurrentNested: ({ port }) =>
      Effect.all(
        [
          parent(generator(port, "nested-a")),
          parent(generator(port, "nested-b")).pipe(thenSignal(port, "nested-b")),
        ],
        unbounded,
      ),

    // The server holds "held" until it has answered the unscoped sibling request.
    outsideScopeCall: ({ port }) =>
      Effect.all([generator(port, "held"), get(port, "outside")], unbounded),

    // The completed sibling's body waits on a timer, whose sleep belongs to that sibling.
    // The response encoder makes a host call; replay of the completed operation skips it.
    encoderCall: ({ port }) =>
      Durability.wrapInfallible(
        {
          iface: "golem-it",
          function: "test-generator",
          functionType: Durability.FunctionType.writeRemote,
          requestSchema: NoInput,
          success: Schema.String.pipe(
            Schema.decodeTo(Schema.String, {
              decode: SchemaGetter.passthrough(),
              encode: SchemaGetter.transformEffect((value: string) =>
                get(port, "encoded").pipe(Effect.as(value)),
              ),
            }),
          ),
        },
        {},
        get(port, "encoder-body"),
      ).pipe(Effect.map((result) => [result])),

    concurrentFirstFinishes: ({ port }) =>
      Effect.all(
        [
          durable("test-generator", getAfterTimer(port, "first")).pipe(
            Effect.tap(() => get(port, "release")),
          ),
          generator(port, "second"),
        ],
        unbounded,
      ),

    failureAndInterruption: ({ port }) =>
      Effect.gen(function* () {
        const failed = yield* Durability.wrap(
          {
            iface: "golem-it",
            function: "test-failing",
            functionType: Durability.FunctionType.writeRemote,
            requestSchema: NoInput,
            success: Generated,
            error: GeneratorFailure,
          },
          {},
          get(port, "failing").pipe(
            Effect.flatMap((result) => Effect.fail({ code: `failed:${result}` })),
          ),
        ).pipe(
          Effect.match({
            onFailure: (error) => error.code,
            onSuccess: ({ result }) => result,
          }),
        );
        // The server holds "interrupted" forever; the race interrupts its body while it waits.
        const raced = yield* Effect.raceFirst(
          generator(port, "interrupted"),
          get(port, "cancel"),
        );
        const after = yield* generator(port, "after-cancel");
        return [failed, raced, after];
      }),

    // A detached fiber started inside the body makes its host call after the scope closed.
    escapedWork: ({ port }) =>
      Effect.gen(function* () {
        const closed = yield* Deferred.make<void>();
        const done = yield* Deferred.make<string>();
        const result = yield* parent(
          Effect.gen(function* () {
            const body = yield* get(port, "escape-body");
            yield* Effect.forkDetach(
              Deferred.await(closed).pipe(
                Effect.andThen(get(port, "escaped")),
                Effect.flatMap((escaped) => Deferred.succeed(done, escaped)),
              ),
            );
            return body;
          }),
        );
        // Resumes the detached fiber synchronously inside this fiber's step.
        yield* Deferred.succeed(closed, undefined);
        const escaped = yield* Deferred.await(done);
        const after = yield* generator(port, "after-escape");
        return [result, escaped, after];
      }),

    // The parent body returns while its detached child operation is still open.
    survivingChild: ({ port }) =>
      Effect.gen(function* () {
        const childStarted = yield* Deferred.make<void>();
        const bodyReturned = yield* Deferred.make<void>();
        const childDone = yield* Deferred.make<string>();
        const [parentResult] = yield* Effect.all(
          [
            parent(
              Effect.gen(function* () {
                yield* Effect.forkDetach(
                  durable(
                    "test-generator",
                    Deferred.succeed(childStarted, undefined).pipe(
                      Effect.andThen(get(port, "survivor")),
                    ),
                  ).pipe(Effect.flatMap((child) => Deferred.succeed(childDone, child))),
                );
                yield* Deferred.await(childStarted);
                yield* Deferred.succeed(bodyReturned, undefined);
                return "parent-body";
              }),
            ),
            // Sent only after the parent body returned; the server holds "survivor" until then.
            Deferred.await(bodyReturned).pipe(Effect.andThen(get(port, "root-probe"))),
          ],
          unbounded,
        );
        const child = yield* Deferred.await(childDone);
        return [parentResult, child];
      }),
  }),
});
