import { LanguageModel } from "effect/ai"
import { AnthropicLanguageModel } from "@effect/ai-anthropic"
import { OpenAiLanguageModel } from "@effect/ai-openai"
const Review = Schema.Struct({
  ok: Schema.Boolean, notes: Schema.String,
})
const Draft = Schema.Struct({ text: Schema.String, review: Review })

export const Drafter = defineAgent({
  name: "DraftAgent",
  id: { topic: Schema.String },
  methods: { write: method({
    input: { idea: Schema.String }, success: Draft,
  }) },
}).implement({
  init: ({ topic }) => Effect.succeed({ topic }),
  methods: ({ topic }) => ({
    write: ({ idea }) => Effect.gen(function* () {
      // draft with one provider, review with another:
      const draft = yield* LanguageModel.generateText({
        prompt: `Product update on ${topic}: ${idea}`,
      }).pipe(Effect.provide(OpenAiLanguageModel.model("gpt-4.1"))) //@
      const { value } = yield* LanguageModel.generateObject({
        schema: Review,
        prompt: `Review this product update:\n${draft.text}`,
      }).pipe(Effect.provide(
        AnthropicLanguageModel.model("claude-sonnet-4-5"))) //@
      return { text: draft.text, review: value }
    }).pipe(Effect.provide(providers), Effect.orDie),
  }),
})
