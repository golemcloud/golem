import { Effect, Option, Schema } from "effect"
import { defineAgent, method, Method, Unstructured } from "../src/index.js"
import type { BinaryReferenceValue, TextReferenceValue } from "../src/Unstructured.js"

type AssertEqual<X, Y> =
  (<T>() => T extends X ? 1 : 2) extends <T>() => T extends Y ? 1 : 2 ? true : false

const Pending = Schema.Struct({ _tag: Schema.Literal("Pending") })

const binaryWithError = method({
  input: {},
  success: Unstructured.UnstructuredBinary(),
  error: Pending,
})
const textWithError = method({
  input: {},
  success: Unstructured.UnstructuredText(),
  error: Pending,
})
const optionalBinary = method({
  input: {},
  success: Unstructured.Option(Unstructured.UnstructuredBinary()),
})
const optionalTextWithError = method({
  input: {},
  success: Unstructured.Option(Unstructured.UnstructuredText()),
  error: Pending,
})

const _binary: AssertEqual<
  Method.MethodSuccessType<typeof binaryWithError.success>,
  BinaryReferenceValue
> = true
const _text: AssertEqual<
  Method.MethodSuccessType<typeof textWithError.success>,
  TextReferenceValue
> = true
const _optionalBinary: AssertEqual<
  Method.MethodSuccessType<typeof optionalBinary.success>,
  Option.Option<BinaryReferenceValue>
> = true
const _optionalText: AssertEqual<
  Method.MethodSuccessType<typeof optionalTextWithError.success>,
  Option.Option<TextReferenceValue>
> = true
const _error: AssertEqual<typeof optionalTextWithError.error, typeof Pending> = true
void [_binary, _text, _optionalBinary, _optionalText, _error]

method({
  // @ts-expect-error optional unstructured elements are success-only, not input parameters
  input: { body: Unstructured.Option(Unstructured.UnstructuredBinary()) },
  success: Schema.Void,
})

const Downloads = defineAgent({
  name: "Downloads",
  id: {},
  methods: { binaryWithError, textWithError, optionalBinary, optionalTextWithError },
})

Downloads.implement({
  init: () => Effect.void,
  methods: () => ({
    binaryWithError: () =>
      Effect.succeed<BinaryReferenceValue>({ _tag: "url", val: "https://example.com" }),
    textWithError: () => Effect.succeed<TextReferenceValue>({ _tag: "inline", val: "ready" }),
    optionalBinary: () => Effect.succeed(Option.none()),
    optionalTextWithError: () => Effect.fail({ _tag: "Pending" as const }),
  }),
})

Downloads.implement({
  init: () => Effect.void,
  methods: () => ({
    binaryWithError: () =>
      Effect.succeed<BinaryReferenceValue>({ _tag: "url", val: "https://example.com" }),
    textWithError: () => Effect.fail({ _tag: "Pending" as const }),
    optionalBinary: () =>
      // @ts-expect-error an optional element handler returns an Option, not a bare reference
      Effect.succeed<BinaryReferenceValue>({ _tag: "url", val: "https://example.com" }),
    // @ts-expect-error the typed error schema constrains handler failures
    optionalTextWithError: () => Effect.fail({ _tag: "Other" as const }),
  }),
})

const program = Effect.scoped(
  Effect.gen(function* () {
    const remote = yield* Downloads.client.get({})
    const found: Option.Option<TextReferenceValue> = yield* remote
      .optionalTextWithError({})
      .pipe(Effect.catchTag("Pending", () => Effect.succeed(Option.none<TextReferenceValue>())))
    return found
  }),
)
void program
