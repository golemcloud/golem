type MessageCallback = (event: { data: string; type: string }) => void;

class WebSocketMock {
  private listeners: MessageCallback[] = [];

  static async connect(_url: string): Promise<WebSocketMock> {
    return new WebSocketMock();
  }

  async send(_data: string): Promise<void> {}

  async disconnect(): Promise<void> {}

  addListener(cb: MessageCallback): void {
    this.listeners.push(cb);
  }
}

export default WebSocketMock;
