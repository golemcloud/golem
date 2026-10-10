import http from 'http';
import nodeHttp, { createServer, IncomingMessage, request, get, Agent } from 'node:http';
import bundled from './node-http-imports.cjs';
import { nodeHttpHandler } from '@golemcloud/golem-ts-sdk/http-router';

const servers = [http, nodeHttp, bundled.http, bundled.nodeHttp].map((module) =>
  module.createServer(),
);
for (const server of servers) nodeHttpHandler(server);
(globalThis as any).__golemNodeHttpImports = {
  http,
  nodeHttp,
  bundled,
  createServer,
  IncomingMessage,
  request,
  get,
  Agent,
};
