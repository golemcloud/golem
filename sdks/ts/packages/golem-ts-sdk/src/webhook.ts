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

import { createWebhook as createWebhookHost, PromiseId } from 'golem:agent/host@1.5.0';
import { createPromise, getPromise, GetPromiseResult } from 'golem:api/host@1.5.0';
import { awaitPromise } from './host/hostapi';

export function createWebhook(): WebhookHandler {
  const promiseId: PromiseId = createPromise();

  const webhookUrl = createWebhookHost(promiseId);

  return new WebhookHandler(webhookUrl, promiseId);
}

export class WebhookHandler implements PromiseLike<WebhookRequestPayload> {
  private readonly url: string;
  private readonly promiseId: PromiseId;

  constructor(url: string, promiseId: PromiseId) {
    this.url = url;
    this.promiseId = promiseId;
  }

  public getUrl(): string {
    return this.url;
  }

  private async wait(): Promise<WebhookRequestPayload> {
    const bytes = await awaitPromise(this.promiseId);

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
