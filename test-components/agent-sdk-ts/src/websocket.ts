import { z } from 'zod';
import { connectWebsocket, defineAgent, method, WebsocketError, type WebSocketHandle } from '@golemcloud/golem-ts-sdk';

export const WebSocketTest = defineAgent({
    name: 'WebSocketTest',
    id: { name: z.string() },
    methods: {
        echo: method({ input: { url: z.string(), msg: z.string() }, returns: z.string() }),
        connectReportLoss: method({ input: { url: z.string() }, returns: z.string() }),
        probeReportLoss: method({ input: {}, returns: z.array(z.boolean()) }),
        replaceLost: method({ input: { url: z.string() }, returns: z.string() }),
    },
});

export const WebSocketTestImpl = WebSocketTest.implement({
    init: () => ({ connection: undefined as WebSocketHandle | undefined }),
    methods: {
        async connectReportLoss({ url }) {
            const connection = await connectWebsocket(url, { reconstructionPolicy: 'report-connection-loss' });
            const first = await connection.receive();
            if (first.tag !== 'text') throw new Error('expected text greeting');
            this.connection = connection;
            return first.val;
        },
        async probeReportLoss() {
            const connection = this.connection;
            if (!connection) throw new Error('connection not initialized');
            const lost = async (operation: () => unknown) => {
                try { await operation(); return false; }
                catch (error) { return error instanceof WebsocketError && error.tag === 'session-lost'; }
            };
            return Promise.all([
                lost(() => connection.receive()),
                lost(() => connection.receiveWithTimeout(0)),
                lost(() => connection.send('must-not-send')),
                lost(() => connection.close()),
            ]);
        },
        async replaceLost({ url }) {
            const old = this.connection;
            if (!old) throw new Error('connection not initialized');
            try { old.send('must-not-send'); throw new Error('expected session loss'); }
            catch (error) {
                if (!(error instanceof WebsocketError) || error.tag !== 'session-lost') throw error;
            }
            const connection = await connectWebsocket(url, { reconstructionPolicy: 'report-connection-loss' });
            this.connection = connection;
            connection.send('initialize-new');
            const message = await connection.receive();
            if (message.tag !== 'text') throw new Error('expected text greeting');
            return message.val;
        },
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
