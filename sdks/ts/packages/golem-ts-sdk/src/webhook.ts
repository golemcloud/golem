// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

import { AgentWebhook, createWebhook as createWebhookHost } from 'golem:agent/host';
import { getPromise, GetPromiseResult } from 'golem:api/host@1.3.0';

export function createWebhook(): WebhookHandler {
  const agentWebhook = createWebhookHost();
  const url = agentWebhook.getCallbackUrl();
  return new WebhookHandler(url, agentWebhook);
}

export class WebhookHandler implements PromiseLike<WebhookRequestPayload> {
  private readonly url: string;
  private readonly inner: AgentWebhook;

  constructor(url: string, inner: AgentWebhook) {
    this.url = url;
    this.inner = inner;
  }

  public getUrl(): string {
    return this.url;
  }

  private async wait(): Promise<WebhookRequestPayload> {
    const trimmed = this.url.replace(/\/+$/, '');
    const parts = trimmed.split('/');
    const promiseIdStr = parts.pop();

    if (!promiseIdStr) {
      throw new Error(`Internal Error: Invalid webhook URL: ${this.url}`);
    }

    // TODO; no easier way to get promise-id from string

    const promiseResult: GetPromiseResult = getPromise(promiseId);

    const pollable = this.inner.subscribe();

    await pollable.promise();

    const bytes = promiseResult.get();

    if (!bytes) {
      throw new Error('Failed to get webhook request payload');
    }

    return new WebhookRequestPayload(bytes);
  }

  then<TResult1 = WebhookRequestPayload, TResult2 = never>(
    onfulfilled?: ((value: WebhookRequestPayload) => TResult1 | PromiseLike<TResult1>) | null,
    onrejected?: ((reason: any) => TResult2 | PromiseLike<TResult2>) | null,
  ): Promise<TResult1 | TResult2> {
    return this.wait().then(onfulfilled, onrejected);
  }
}

export class WebhookRequestPayload {
  private readonly payload: Uint8Array;

  constructor(payload: Uint8Array) {
    this.payload = payload;
  }

  public json<T>(): T {
    try {
      const text = new TextDecoder().decode(this.payload);
      return JSON.parse(text) as T;
    } catch (e) {
      throw new Error(`Invalid input: ${String(e)}`);
    }
  }

  public bytes(): Uint8Array {
    return this.payload;
  }
}
