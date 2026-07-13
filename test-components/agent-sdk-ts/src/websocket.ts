import { z } from 'zod';
import { defineAgent, method } from '@golemcloud/golem-ts-sdk';

export const WebSocketTest = defineAgent({
    name: 'WebSocketTest',
    id: { name: z.string() },
    methods: {
        echo: method({ input: { url: z.string(), msg: z.string() }, returns: z.string() }),
    },
});

export const WebSocketTestImpl = WebSocketTest.implement({
    init: () => ({}),
    methods: {
        echo({ url, msg }) {
            return new Promise<string>((resolve, reject) => {
                const ws = new WebSocket(url);
                ws.onopen = () => ws.send(msg);
                ws.onmessage = (event) => { ws.close(); resolve(event.data); };
                ws.onerror = (event) => reject(new Error(event.message));
            });
        },
    },
});
