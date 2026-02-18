import {
  BaseAgent,
  agent,
  prompt,
  description,
  endpoint
} from '@golemcloud/golem-ts-sdk';
import * as llm from 'golem:llm/llm@1.0.0';
import * as webSearch from 'golem:web-search/web-search@1.0.0';
import { env } from 'node:process';

type SearchResult = {
  url: string;
  title: string;
  snippet: string
}

@agent({
  mount: "/research",
  phantom: true
})
class ResearchAgent extends BaseAgent {
  private readonly model: string;

  constructor() {
    super()

    {
      const model = env["LLM_MODEL"];
      if (model == null) {
        throw "No LLM_MODEL env var provided"
      }
      this.model = model
    }

    // check that the user configured the agent
    {
      const googleApiKey = env["GOOGLE_API_KEY"]
      if (googleApiKey == null || googleApiKey === "changeme") {
        throw "GOOGLE_API_KEY env var not configured. Check the golem.yaml for instructions"
      }
    }
    {
      const googleSearchEngineId = env["GOOGLE_SEARCH_ENGINE_ID"]
      if (googleSearchEngineId == null || googleSearchEngineId === "changeme") {
        throw "GOOGLE_SEARCH_ENGINE_ID env var not configured. Check the golem.yaml for instructions"
      }
    }
  }

  @prompt("What topic do you want to research?")
  @description("This method allows the agent to research and summarize a topic for you")
  @endpoint({ get: "/?topic={topic}" })
  async research(topic: string): Promise<string> {
    const searchResult = searchWebForTopic(topic)

    let llmResult = llm.send(
      [
        {
          tag: "message",
          val: {
            role: "assistant",
            name: "research-agent",
            content: [
              {
                tag: "text",
                val: `
                  I'm writing a report on the topic "${ topic }",
                  Your job is to be a research-assistant and provide me an initial overview on the topic so I can dive into it in more detail.
                  At the bottom are top search results from a search engine in json format. Use your own knowledge and the snippets from the search results to create the overview.
                  Also include the best links to look into to learn more about the topic. Prioritize objective and reliable sources.

                  Search results: ${ JSON.stringify(searchResult) }
                `
              }
            ]
          }
        }
      ],
      {
        model: this.model,
        tools: [],
        toolChoice: undefined,
        stopSequences: undefined,
        maxTokens: undefined,
        temperature: undefined,
        providerOptions: []
      }
    );

    const textResult = llmResult.content.filter(content => content.tag === "text").map(content => content.val).join("\n");

    return `Finished research for topic ${ topic }:\n${ textResult }`
  }
}

function searchWebForTopic(topic: string): SearchResult[] {
  // get 30 results in total
  const pagesToRetrieve = 3

  const session = webSearch.startSearch({
    query: topic,
    language: "lang_en",
    safeSearch: "off",
    maxResults: 10,
    advancedAnswer: true
  })

  const content: SearchResult[] = []

  for (let i = 0; i < pagesToRetrieve; i++) {
    const page = session.nextPage()

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
