import {
  BaseAgent,
  agent,
  description,
  endpoint
} from '@golemcloud/golem-ts-sdk';
import * as llm from 'golem:llm/llm@1.0.0';

@agent({
  mount: "/chats/{chatName}"
})
class ChatAgent extends BaseAgent {
  private readonly chatName: string;
  private session: LlmSession;

  constructor(chatName: string) {
    super()
    this.chatName = chatName;
    this.session = new LlmSession({
      model: "gpt-oss:20b",
    });
    this.session.addMessage({
      role: "system",
      content: [{
        tag: "text",
        val: `You are a helpful and very funny assistant for a chat named ${ chatName }.`,
      }]
    });
  }

  @description("Ask questions")
  @endpoint({ post: "/ask" })
  async ask(question: string): Promise<string> {
    this.session.addMessage({
      role: "user",
      content: [{
        tag: "text",
        val: question,
      }],
    });
    let response = this.session.send();
    return response.content
      .filter(contentPart => contentPart.tag === "text")
      .map(contentPart => contentPart.val)
      .join("\n");
  }

  @description("Show full chat history")
  @endpoint({ get: "/history" })
  async history(): Promise<llm.Event[]> {
    return this.session.events
  }
}

class LlmSession {
  config: llm.Config;
  events: llm.Event[];

  constructor(config: llm.Config) {
    this.config = config;
    this.events = [];
  }

  addMessage(message: llm.Message) {
    this.events.push({
      tag: "message",
      val: message,
    });
  }

  addToolResult(toolResult: llm.ToolResult) {
    this.events.push({
      tag: "tool-results",
      val: [toolResult]
    })
  }

  addToolResults(toolResults: llm.ToolResult[]) {
    this.events.push({
      tag: "tool-results",
      val: toolResults
    });
  }

  send(): llm.Response {
    let response = llm.send(this.events, this.config);
    this.events.push({
      tag: "response",
      val: response
    });
    return response;
  }
}
