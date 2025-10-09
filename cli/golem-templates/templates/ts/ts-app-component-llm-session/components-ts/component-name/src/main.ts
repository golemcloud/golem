import {
  BaseAgent,
  agent,
  prompt,
  description,
} from '@golemcloud/golem-ts-sdk';
import * as llm from 'golem:llm/llm@1.0.0';

@agent()
class ChatAgent extends BaseAgent {
  private readonly chatName: string;
  private session: LlmSession;

  constructor(chatName: string) {
    super()
    this.chatName = chatName;
    this.session = new LlmSession({
      model: "gpt-oss:20b",
    });
  }

  @description("Ask questions")
  async ask(question: string): Promise<string> {
    this.session.addMessage(question);
    let response = this.session.send();
  }

  @description("Show full chat history")
  async history(question: string): Promise<llm.ChatEvent> {
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
