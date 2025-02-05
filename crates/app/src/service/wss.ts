import WebSocket from "@tauri-apps/plugin-websocket";

export class WSS {
  private ws: WebSocket;

  constructor(ws: WebSocket) {
    this.ws = ws;
  }

  static async getConnection(url: string) {
    return new WSS(await WebSocket.connect(url));
  }

  public send(data: never) {
    this.ws
      .send(JSON.stringify(data))
      .then(() => {})
      .catch(console.error);
  }

  public close() {
    this.ws
      .disconnect()
      .then(() => {})
      .catch(console.error);
  }

  public onMessage(callback: (data: unknown) => void) {
    this.ws.addListener((event) => {
      const message = event.data;
      try {
        if (typeof message === "string") {
          callback(JSON.parse(message));
        }
      } catch (e) {
        console.error("Failed to parse message", e);
      }
    });
  }
}
