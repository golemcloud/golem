import { agent, BaseAgent } from "@golemcloud/golem-ts-sdk";

@agent()
class WebSocketTest extends BaseAgent {
    constructor(name: string) {
        super();
    }

    async echo(url: string, msg: string): Promise<string> {
        return new Promise((resolve, reject) => {
            const ws = new WebSocket(url);
            ws.onopen = () => ws.send(msg);
            ws.onmessage = (event) => { ws.close(); resolve(event.data); };
            ws.onerror = (event) => reject(new Error(event.message));
        });
    }
}