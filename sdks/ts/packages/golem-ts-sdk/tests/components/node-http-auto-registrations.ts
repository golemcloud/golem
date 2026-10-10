import { createServer } from 'node:http';
import { defineHttpRouter, nodeHttpHandler } from '@golemcloud/golem-ts-sdk/http-router';

(globalThis as any).__golemAutoAction({ createServer, defineHttpRouter, nodeHttpHandler });
