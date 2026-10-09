// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

import * as originalHttp from 'node:http';
import { EventEmitter } from 'node:events';
import { Writable } from 'node:stream';
import { Buffer } from 'node:buffer';
import { defineHttpRouter, type RawHttpRouterHandler } from './defineHttpRouter';
import type { HttpHeader, HttpRequest, HttpResponse } from './httpRouterContract';
import { AgentStream, disposeAgentStream } from './schema/agentStream';
import { AgentClassName } from './agentClassName';
import { AgentTypeRegistry } from './internal/registry/agentTypeRegistry';
import {
  completeNodeHttpRegistration,
  nodeHttpRouterForPort,
} from './internal/http/nodeHttpRegistration';

export * from 'node:http';

type RequestListener = (request: originalHttp.IncomingMessage, response: ServerResponse) => void;
const registrations = new WeakMap<Server, 'explicit' | 'automatic'>();

/** A local request listener container, not a network server. */
export class Server extends EventEmitter {
  constructor(listener?: RequestListener) {
    super();
    if (arguments.length > 1 || (listener !== undefined && typeof listener !== 'function'))
      throw new TypeError(
        'createServer supports only a request listener; server options are unsupported',
      );
    if (listener) this.on('request', listener);
  }

  get listening() {
    return registrations.get(this) === 'automatic';
  }

  listen(port?: number, callback?: () => void): this {
    if (
      arguments.length > 2 ||
      !Number.isInteger(port) ||
      typeof port !== 'number' ||
      port < 1 ||
      port > 65535 ||
      (callback !== undefined && typeof callback !== 'function')
    )
      throw new TypeError('listen() supports only a numeric configured port and optional callback');
    if (registrations.has(this))
      throw new Error('Server already has an explicit or automatic HTTP registration');
    const router = nodeHttpRouterForPort(port);
    const name = new AgentClassName(router.name);
    if (AgentTypeRegistry.exists(name))
      throw new Error(`Router "${router.name}" is already registered`);
    defineHttpRouter(router.name)
      .mount(router.mount, { auth: router.auth, cors: router.cors })
      .implementRaw(createNodeHttpHandler(this));
    const diagnostic = AgentTypeRegistry.getRegistrationError(router.name);
    if (diagnostic?.length || !AgentTypeRegistry.get(name))
      throw new Error(diagnostic?.join('; ') ?? 'HTTP router registration failed');
    registrations.set(this, 'automatic');
    completeNodeHttpRegistration(port);
    if (callback) this.once('listening', callback);
    globalThis.queueMicrotask(() => this.emit('listening'));
    return this;
  }

  address(): null {
    return null;
  }

  close(): never {
    throw new Error(
      'close() cannot undeploy a Golem HTTP route; change the HTTP API deployment instead',
    );
  }
}

export function createServer(listener?: RequestListener): Server {
  if (arguments.length > 1)
    throw new TypeError(
      'createServer supports only a request listener; server options are unsupported',
    );
  return new Server(listener);
}

class RouterIncomingMessage extends originalHttp.IncomingMessage {
  private reading = false;

  constructor(
    private readonly request: HttpRequest<AgentStream<Uint8Array>>,
    private readonly onDestroy: (error: Error | null) => void,
  ) {
    // The runtime accepts null for socket-free requests. No native client body is attached.
    super(null as unknown as import('node:net').Socket);
    this.method = request.method;
    this.url = request.path + (request.query === undefined ? '' : `?${request.query}`);
    for (const field of ['httpVersion', 'httpVersionMajor', 'httpVersionMinor'])
      Reflect.deleteProperty(this, field);
    this.headers = Object.create(null);
    this.rawHeaders = [];
    const distinct: Record<string, string[]> = Object.create(null);
    for (const header of request.headers) {
      const name = header.name.toLowerCase();
      const value = Buffer.from(header.value).toString('latin1');
      this.rawHeaders.push(header.name, value);
      (distinct[name] ??= []).push(value);
      const prior = this.headers[name];
      if (name === 'set-cookie') this.headers[name] = [...((prior as string[]) ?? []), value];
      else if (prior === undefined) this.headers[name] = value;
      else if (name === 'cookie') this.headers[name] = `${prior}; ${value}`;
      else if (
        ![
          'age',
          'authorization',
          'content-length',
          'content-type',
          'etag',
          'expires',
          'from',
          'host',
          'if-modified-since',
          'if-unmodified-since',
          'last-modified',
          'location',
          'max-forwards',
          'proxy-authorization',
          'referer',
          'retry-after',
          'server',
          'user-agent',
        ].includes(name)
      )
        this.headers[name] = `${prior}, ${value}`;
    }
    this.headers.host = request.authority;
    this.rawHeaders.push('host', request.authority);
    distinct.host = [request.authority];
    Object.defineProperty(this, 'headersDistinct', { value: distinct, configurable: true });
  }

  override _read() {
    if (this.reading || this.destroyed) return;
    this.reading = true;
    void this.request.body.next().then(
      (item) => {
        this.reading = false;
        if (this.destroyed) return;
        if (item.done) {
          this.complete = true;
          this.push(null);
        } else if (item.value.length === 0) this._read();
        else this.push(Buffer.from(item.value));
      },
      (error) => this.destroy(error instanceof Error ? error : new Error(String(error))),
    );
  }

  override _destroy(error: Error | null, callback: (error?: Error | null) => void) {
    if (!this.complete && !this.aborted) {
      this.aborted = true;
      this.onDestroy(error);
      this.emit('aborted');
    }
    void disposeAgentStream(this.request.body).then(
      () => callback(error),
      (failure) => callback(error ?? failure),
    );
  }
}

/** HTTP response metadata over a demand-driven writable body. */
export class ServerResponse extends Writable {
  statusCode = 200;
  private head?: { status: number; headers: HttpHeader[] };
  private readonly fields = new Map<string, string | number | readonly string[]>();
  private pending?: { chunk: Uint8Array; callback: (error?: Error | null) => void };
  private wake?: () => void;
  private ended = false;
  private disposed = false;
  private failure?: Error;

  constructor(private readonly commit: () => void) {
    super();
  }

  get headersSent() {
    return this.head !== undefined;
  }
  get statusMessage() {
    return '';
  }
  set statusMessage(_value: string) {
    throw new Error('Custom reason phrases are unsupported');
  }

  setHeader(name: string, value: string | number | readonly string[]) {
    if (this.headersSent) throw new Error('Response headers are already committed');
    originalHttp.validateHeaderName(name);
    for (const item of Array.isArray(value) ? value : [value])
      originalHttp.validateHeaderValue(name, item);
    this.fields.set(name.toLowerCase(), Array.isArray(value) ? [...value] : value);
    return this;
  }

  getHeader(name: string) {
    return this.fields.get(name.toLowerCase());
  }
  getHeaders() {
    return Object.fromEntries(this.fields);
  }
  getHeaderNames() {
    return [...this.fields.keys()];
  }
  hasHeader(name: string) {
    return this.fields.has(name.toLowerCase());
  }
  removeHeader(name: string) {
    if (this.headersSent) throw new Error('Response headers are already committed');
    this.fields.delete(name.toLowerCase());
  }

  writeHead(
    status: number,
    headers?: originalHttp.OutgoingHttpHeaders | readonly string[] | string,
  ) {
    if (typeof headers === 'string') throw new Error('Custom reason phrases are unsupported');
    if (this.headersSent) throw new Error('Response headers are already committed');
    this.statusCode = status;
    if (Array.isArray(headers)) {
      if (headers.length % 2)
        throw new TypeError('Raw response headers must contain name/value pairs');
      const seen = new Set<string>();
      for (let i = 0; i < headers.length; i += 2) {
        const name = headers[i].toLowerCase();
        const value = headers[i + 1];
        const previous = seen.has(name) ? this.getHeader(name) : undefined;
        this.setHeader(
          name,
          previous === undefined
            ? value
            : [...(Array.isArray(previous) ? previous : [String(previous)]), value],
        );
        seen.add(name);
      }
    } else {
      for (const [name, value] of Object.entries(headers ?? {}))
        if (value !== undefined) this.setHeader(name, value);
    }
    this.flushHeaders();
    return this;
  }

  flushHeaders() {
    if (this.headersSent) return;
    if (!Number.isInteger(this.statusCode) || this.statusCode < 200 || this.statusCode > 599)
      throw new RangeError('Response status must be between 200 and 599');
    const headers: HttpHeader[] = [];
    for (const [name, value] of this.fields)
      for (const item of Array.isArray(value) ? value : [value]) {
        originalHttp.validateHeaderValue(name, item);
        headers.push({ name, value: Buffer.from(String(item), 'latin1') });
      }
    this.head = { status: this.statusCode, headers };
    this.commit();
  }

  addTrailers(): never {
    throw new Error('HTTP trailers are unsupported');
  }
  writeContinue(): never {
    throw new Error('Informational responses are unsupported');
  }
  writeEarlyHints(): never {
    throw new Error('Informational responses are unsupported');
  }
  writeProcessing(): never {
    throw new Error('Informational responses are unsupported');
  }

  suppressesBody(method: string) {
    return (
      method === 'HEAD' ||
      [204, 205, 304].includes(this.head!.status) ||
      this.head!.headers.some(
        ({ name, value }) =>
          name === 'content-length' &&
          /^[ \t]*0+[ \t]*$/.test(Buffer.from(value).toString('latin1')),
      )
    );
  }

  disposeBody(error?: Error) {
    this.failure ??= error;
    this.disposed = true;
    const pending = this.pending;
    this.pending = undefined;
    this.wake?.();
    this.wake = undefined;
    pending?.callback(error);
  }

  envelope(
    close: (error?: Error) => Promise<void>,
    method: string,
  ): HttpResponse<AgentStream<Uint8Array>> {
    if (!this.head) throw new Error('Response headers are not committed');
    const response = this;
    return {
      ...this.head,
      body: AgentStream.from<Uint8Array>({
        [Symbol.asyncIterator]: () => ({
          async next() {
            try {
              // The native producer can pull before the host drops a bodyless reader.
              if (response.suppressesBody(method)) {
                await close();
                if (response.failure) throw response.failure;
                return { done: true as const, value: undefined };
              }
              while (
                !response.pending &&
                !response.ended &&
                !response.disposed &&
                !response.failure
              )
                await new Promise<void>((resolve) => {
                  response.wake = resolve;
                });
              if (response.failure) throw response.failure;
              const pending = response.pending;
              response.pending = undefined;
              if (!pending) {
                await close();
                if (response.failure) throw response.failure;
                return { done: true as const, value: undefined };
              }
              pending.callback();
              return { done: false as const, value: pending.chunk };
            } catch (error) {
              const failure = error instanceof Error ? error : new Error(String(error));
              await close(failure).catch(() => undefined);
              throw failure;
            }
          },
          async return() {
            if (response.failure) {
              await close(response.failure).catch(() => undefined);
              throw response.failure;
            }
            const cancellation =
              response.ended || response.suppressesBody(method)
                ? undefined
                : new Error('Response body was disposed');
            await close(cancellation);
            if (response.failure && response.failure !== cancellation) throw response.failure;
            return { done: true as const, value: undefined };
          },
        }),
      }),
    };
  }

  override _write(chunk: Buffer, _encoding: string, callback: (error?: Error | null) => void) {
    try {
      this.flushHeaders();
    } catch (error) {
      callback(error as Error);
      return;
    }
    if (this.disposed) {
      callback();
      return;
    }
    this.pending = { chunk: Buffer.from(chunk), callback };
    this.wake?.();
    this.wake = undefined;
  }

  override _final(callback: (error?: Error | null) => void) {
    try {
      this.flushHeaders();
    } catch (error) {
      callback(error as Error);
      return;
    }
    this.ended = true;
    this.wake?.();
    callback();
  }

  override _destroy(error: Error | null, callback: (error?: Error | null) => void) {
    this.failure =
      error ??
      (!this.ended && !this.disposed
        ? new Error('Response destroyed before completion')
        : undefined);
    this.disposeBody(this.failure);
    callback(this.failure);
  }
}

export function nodeHttpHandler(server: Server | originalHttp.Server): RawHttpRouterHandler {
  if (!(server instanceof Server))
    throw new TypeError('nodeHttpHandler requires createServer from a Golem component build');
  if (registrations.get(server) === 'automatic')
    throw new Error('An automatically registered server cannot also use nodeHttpHandler');
  registrations.set(server, 'explicit');
  return createNodeHttpHandler(server);
}

function createNodeHttpHandler(server: Server): RawHttpRouterHandler {
  return async (raw) => {
    let resolve!: () => void;
    let reject!: (error: Error) => void;
    const headers = new Promise<void>((yes, no) => {
      resolve = yes;
      reject = no;
    });
    const request = new RouterIncomingMessage(raw, (error) => {
      if (!cleanup) fail(error ?? new Error('Request destroyed before completion'));
    });
    const response = new ServerResponse(resolve);
    let cleanup: Promise<void> | undefined;
    let failure: Error | undefined;
    let requestFailure: Error | undefined;
    const requestClosed = new Promise<void>((done) => request.once('close', done));
    const close = (error?: Error) => {
      if (error) reject(error);
      return (cleanup ??= Promise.resolve().then(async () => {
        try {
          response.disposeBody(error);
        } finally {
          request.destroy(error);
          if (error) response.destroy(error);
        }
        await requestClosed;
        if (requestFailure && requestFailure !== error) throw requestFailure;
      }));
    };
    const fail = (error: Error) => {
      failure ??= error;
      reject(failure);
      void close(failure).catch(() => undefined);
      response.destroy(failure);
    };
    request.on('error', (error) => {
      requestFailure = error;
      fail(error);
    });
    response.on('error', fail);
    try {
      if (!server.emit('request', request, response))
        throw new Error('Server has no request listener');
    } catch (error) {
      fail(error instanceof Error ? error : new Error(String(error)));
    }
    try {
      await headers;
      if (failure) throw failure;
      return response.envelope(close, raw.method);
    } catch (error) {
      await close(error instanceof Error ? error : new Error(String(error))).catch(() => undefined);
      throw error;
    }
  };
}

export default { ...originalHttp, Server, ServerResponse, createServer };
