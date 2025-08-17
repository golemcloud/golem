export interface Api {
  createdAt?: string;
  draft: boolean;
  id: string;
  routes: RouteRequestData[];
  version: string;
  count?: number;
}

export type MethodPattern =
  | "GET"
  | "CONNECT"
  | "POST"
  | "DELETE"
  | "PUT"
  | "PATCH"
  | "OPTIONS"
  | "TRACE"
  | "HEAD";
export type GatewayBindingType = "default" | "file-server" | "cors-preflight";

export interface RouteRequestData {
  method: MethodPattern;
  path: string;
  binding: GatewayBindingData;
  cors?: HttpCors;
  security?: string;
}

export interface GatewayBindingData {
  bindingType: GatewayBindingType;
  component?: GatewayBindingComponent;
  workerName?: string;
  idempotencyKey?: string;
  response?: string;
  corsPreflight?: HttpCors;
}

export interface GatewayBindingComponent {
  name: string;
  version?: number;
}

export interface HttpCors {
  allowOrigin: string;
  allowMethods: string;
  allowHeaders: string;
  exposeHeaders?: string;
  maxAge?: number;
  allowCredentials?: boolean;
}

export interface HttpApiDefinitionRequest {
  id: string;
  version: string;
  security?: string[];
  routes: RouteRequestData[];
  draft: boolean;
}
