import { VersionedComponentId } from "@/types/component.ts";

export interface Api {
  createdAt?: string;
  draft: boolean;
  id: string;
  routes: RouteRequestData[];
  version: string;
  count?: number;
}

export type MethodPattern =
  | "Get"
  | "Post"
  | "Put"
  | "Delete"
  | "Patch"
  | "Head"
  | "Options"
  | "Trace"
  | "Connect";
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
  componentId?: VersionedComponentId;
  workerName?: string;
  idempotencyKey?: string;
  response?: string;
  corsPreflight?: HttpCors;
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
