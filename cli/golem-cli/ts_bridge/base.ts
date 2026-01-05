
export type PhantomId = string;

export type GolemServer = "local" | "cloud" | { url: string, token: string };

export type Configuration = {
    server: GolemServer,
    application: ApplicationName,
    environment: EnvironmentName,
}

export type ApplicationName = string;
export type EnvironmentName = string;
export type AgentTypeName = string;
export type IdempotencyKey = string;

export type UntypedDataValue = 
  | { type: "tuple"; elements: UntypedElementValue[] }
  | { type: "multimodal"; elements: UntypedNamedElementValue[] };

export type UntypedElementValue = 
  | { type: "componentModel"; value: JsonComponentModelValue }
  | { type: "unstructuredText"; value: TextReference }
  | { type: "unstructuredBinary"; value: BinaryReference };

export interface UntypedNamedElementValue {
  name: string;
  value: UntypedElementValue;
}

export interface JsonComponentModelValue {
  value: unknown;
}

export type TextReference = 
  | { type: "url"; value: string }
  | { type: "inline"; data: string; textType?: TextType };

export interface TextType {
  languageCode: string;
}

export type BinaryReference = 
  | { type: "url"; value: string }
  | { type: "inline"; data: string; binaryType: BinaryType };

export interface BinaryType {
  mimeType: string;
}

export type DataValue = UntypedDataValue;

export type AgentInvocationMode  = "await" | "schedule";

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

export async function invokeAgent(
  server: GolemServer,
  request: AgentInvocationRequest,
): Promise<AgentInvocationResult> {
  const baseUrl = typeof server === "string" 
    ? (server === "local" ? "http://localhost:9080" : "https://api.golem.cloud")
    : server.url;

  const headers: HeadersInit = {
      "Content-Type": "application/json",
  };

  if (typeof server !== "string" && server.token) {
      headers["Authorization"] = `Bearer ${server.token}`;
  }

  if (request.idempotencyKey) {
    headers["Idempotency-Key"] = request.idempotencyKey!;
  }

  const response = await fetch(
    `${baseUrl}/v1/agents/invoke-agent`,
    {
      method: "POST",
      headers,
      body: JSON.stringify(request),
    },
  );

  if (!response.ok) {
    throw new Error(`Agent invocation failed: ${response.statusText}`);
  }

  return await (response.json() as Promise<AgentInvocationResult>);
}

export type JsonResult<Ok, Err> = { ok: Ok } | { err: Err };

export function encodeOption<T>(value: T | undefined, encode: (v: T) => unknown): unknown {
    if (value === undefined) {
        return null;
    } else {
        return encode(value);
    }
}
