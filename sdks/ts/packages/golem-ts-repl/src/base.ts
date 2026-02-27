// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// TODO: import all these from bridge base, once it is extracted into the SDK

export type PhantomId = string;

export type GolemServer =
  | { type: 'local' }
  | { type: 'cloud'; token: string }
  | { type: 'custom'; url: string; token: string };

export type AroundInvokeHook = {
  beforeInvoke: (request: AgentInvocationRequest) => Promise<void>;
  afterInvoke: (
    request: AgentInvocationRequest,
    result: JsonResult<AgentInvocationResult, unknown>,
  ) => Promise<void>;
};

export type Configuration = {
  server: GolemServer;
  application: ApplicationName;
  environment: EnvironmentName;
  aroundInvokeHook?: AroundInvokeHook;
};

export type ApplicationName = string;
export type EnvironmentName = string;
export type AgentTypeName = string;
export type IdempotencyKey = string;

export type Timestamp = string; // ISO 8601

export type WorkerStatus =
  | 'Running'
  | 'Idle'
  | 'Suspended'
  | 'Interrupted'
  | 'Retrying'
  | 'Failed'
  | 'Exited';

export type UpdateRecord =
  | { type: 'PendingUpdate'; timestamp: Timestamp; targetRevision: number }
  | { type: 'SuccessfulUpdate'; timestamp: Timestamp; targetRevision: number }
  | {
      type: 'FailedUpdate';
      timestamp: Timestamp;
      targetRevision: number;
      details?: string;
    };

export type WorkerResourceDescription = {
  createdAt: Timestamp;
  resourceOwner: string;
  resourceName: string;
};

export type Worker = {
  componentName: string;
  workerName: string;
  createdBy: string;
  environmentId: string;
  env: Record<string, string>;
  configVars: Record<string, string>;
  status: WorkerStatus;
  componentRevision: number;
  retryCount: number;
  pendingInvocationCount: number;
  updates: UpdateRecord[];
  createdAt: Timestamp;
  lastError?: string;
  componentSize: number;
  totalLinearMemorySize: number;
  exportedResourceInstances: Record<string, WorkerResourceDescription>;
};

export type DataValue =
  | { type: 'Tuple'; elements: ElementValue[] }
  | { type: 'Multimodal'; elements: NamedElementValue[] };

export type ElementValue =
  | { type: 'ComponentModel'; value: unknown }
  | { type: 'UnstructuredText'; value: TextReference }
  | { type: 'UnstructuredBinary'; value: BinaryReference };

export interface NamedElementValue {
  name: string;
  value: ElementValue;
}

export type Url = {
  value: string;
};

export type TextSource = {
  data: string;
  textType?: TextType;
};

export type TextReference =
  | { type: 'Url'; value: string }
  | { type: 'Inline'; data: string; textType?: TextType };

export const TextReference = {
  fromUnstructuredText<LC extends LanguageCode[]>(input: UnstructuredText<LC>): TextReference {
    if (input.tag === 'url') {
      return {
        type: 'Url',
        value: input.val,
      };
    } else {
      return {
        type: 'Inline',
        data: input.val,
        textType: input.languageCode ? { languageCode: input.languageCode as string } : undefined,
      };
    }
  },
};

export interface TextType {
  languageCode: string;
}

export type BinarySource = {
  data: Uint8Array;
  binaryType: BinaryType;
};

export type BinaryReference =
  | { type: 'Url'; value: string }
  | { type: 'Inline'; data: Uint8Array; binaryType: BinaryType };

export const BinaryReference = {
  fromUnstructuredBinary<MT extends MimeType[] | MimeType>(
    input: UnstructuredBinary<MT>,
  ): BinaryReference {
    if (input.tag === 'url') {
      return {
        type: 'Url',
        value: input.val,
      };
    } else {
      return {
        type: 'Inline',
        data: input.val,
        binaryType: { mimeType: input.mimeType as string },
      };
    }
  },
};

export interface BinaryType {
  mimeType: string;
}

export type AgentInvocationMode = 'await' | 'schedule';

export interface AgentInvocationRequest {
  appName: ApplicationName;
  envName: EnvironmentName;
  agentTypeName: AgentTypeName;
  parameters: DataValue;
  phantomId?: PhantomId;
  methodName: string;
  methodParameters: DataValue;
  mode: AgentInvocationMode;
  scheduleAt?: string; // ISO 8601 datetime
  idempotencyKey?: IdempotencyKey;
}

export interface AgentInvocationResult {
  result?: DataValue;
}

/// The Result type representation in Golem's JSON type mapping
export type JsonResult<Ok, Err> = { ok: Ok; err?: undefined } | { ok?: undefined; err: Err };

type MimeType = string;

export type UnstructuredBinary<MT extends MimeType[] | MimeType = MimeType> =
  | {
      tag: 'url';
      val: string;
    }
  | {
      tag: 'inline';
      val: Uint8Array;
      mimeType: MT extends MimeType[] ? MT[number] : MimeType;
    };

type LanguageCode = string;

export type UnstructuredText<LC extends LanguageCode[] = []> =
  | {
      tag: 'url';
      val: string;
    }
  | {
      tag: 'inline';
      val: string;
      languageCode?: LC[number];
    };
