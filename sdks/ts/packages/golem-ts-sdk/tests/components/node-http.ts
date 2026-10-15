import { createServer } from 'node:http';
import { defineHttpRouter, nodeHttpHandler } from '@golemcloud/golem-ts-sdk/http-router';

const server = createServer((req, res) => {
  res.writeHead(200, { 'content-type': 'text/plain' });
  res.end('Hello from Golem');
});

export const router = defineHttpRouter('Web').mount('/').implementRaw(nodeHttpHandler(server));

(globalThis as any).__golemNodeHttpHandler = nodeHttpHandler(server);
