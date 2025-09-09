import {
  BaseAgent,
  agent,
  prompt,
  description,
  AtomicOperationGuard,
  markAtomicOperation,
  atomically,
  withRetryPolicy,
} from '@golemcloud/golem-ts-sdk';
import {
  send as sendToLLM
} from 'golem:llm/llm@1.0.0'
import { startSearch } from 'golem:web-search/web-search@1.0.0';
import { env } from 'process';

type SearchResult = {
  url: string;
  title: string;
  snippet: string
}

@agent()
class ResearchAgent extends BaseAgent {

  @prompt("What topic do you want to research?")
  @description("This method allows the agent to research and summarize a topic for you")
  async research(topic: string): Promise<string> {
    const model = env["LLM_MODEL"];
    if (model == null) {
      throw "No LLM_MODEL env var provided"
    }

    const searchResult = searchWebForTopic(topic)

    const llmResponse = atomically(() => {
      let result = sendToLLM(
        [
          {
            role: "assistant",
            name: "research-agent",
            content: [
              {
                tag: "text",
                val: `
                  I'm writing a report on the topic "${topic}",
                  Your job is to be a research-assistant and provide me an initial overview on the topic so I can dive into it in more detail.
                  At the bottom are top search results from a search engine in json format. Use your own knowledge and the snippets from the search results to create the overview.
                  Also include the best links to look into to learn more about the topic. Prioritize objective and reliable sources.

                  Search results: ${JSON.stringify(searchResult)}
                `
              }
            ]
          }
        ],
        {
          model: model,
          tools: [],
          toolChoice: undefined,
          stopSequences: undefined,
          maxTokens: undefined,
          temperature: undefined,
          providerOptions: []
        }
      )

      if (result.tag != 'message') {
        throw "Unexpected chatevent tag from llm"
      }

      let content = result.val.content[0]

      if (content == null || content.tag != 'text') {
        throw "Didn't receive expected text response from llm"
      }

      return content.val
    })

    return `Finished research for topic ${topic}:\n${llmResponse}`
  }
}

function searchWebForTopic(topic: string): SearchResult[] {
  // get 30 results in total
  const pagesToRetrieve = 3

  const session = atomically(() => {
    const result = startSearch({
      query: topic,
      language: "lang_en",
      safeSearch: "off",
      region: undefined,
      maxResults: 10,
      timeRange: undefined,
      includeDomains: undefined,
      excludeDomains: undefined,
      includeImages: false,
      includeHtml: false,
      advancedAnswer: true
    })
    if (result.tag != "ok") {
      throw result.val
    }
    return result.val
  })

  const content: SearchResult[] = []

  for (let i = 0; i < pagesToRetrieve; i++) {
    const page = atomically(() => {
      const result = session.nextPage()
      if (result.tag != "ok") {
        throw `Retrieving page ${i} failed: ${JSON.stringify(result.val)}`
      }
      return result.val
    })

    for (let item of page) {
      content.push({
        url: item.url,
        title: item.title,
        snippet: item.snippet
      })
    }
  }

  return content
}
