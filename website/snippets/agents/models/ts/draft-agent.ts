import { generateText, Output } from 'ai'
import { anthropic } from '@ai-sdk/anthropic'
import { openai } from '@ai-sdk/openai'
const Review = z.object({ ok: z.boolean(), notes: z.string() })
const Draft = z.object({ text: z.string(), review: Review })

export const Drafter = defineAgent({
  name: 'DraftAgent',
  id: { topic: z.string() },
  methods: {
    write: method({ input: { idea: z.string() }, returns: Draft }),
  },
}).implement({
  init: ({ id }) => ({ topic: id.topic }),
  methods: {
    async write({ idea }) {
      // draft with one provider, review with another:
      const draft = await generateText({
        model: openai('gpt-4.1'), //@
        prompt: `Product update on ${this.topic}: ${idea}`,
      })
      const { output } = await generateText({
        model: anthropic('claude-sonnet-4-5'), //@
        output: Output.object({ schema: Review }),
        prompt: `Review this product update:\n${draft.text}`,
      })
      return { text: draft.text, review: output }
    },
  },
})
