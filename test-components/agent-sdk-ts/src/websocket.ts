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

@agent()
class WebSocketStreamTest extends BaseAgent {
    constructor(name: string) {
        super();
    }

    async echo(url: string, msg: string): Promise<string> {
        const wss = new WebSocketStream(url);
        const { readable, writable } = await wss.opened;

        const writer = writable.getWriter();
        await writer.write(msg);

        const reader = readable.getReader();
        const { value } = await reader.read();

        wss.close();
        return String(value);
    }
}