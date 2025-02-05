export interface Api {
  createdAt?: string;
  draft: boolean;
  id: string;
  routes: Route[];
  version: string;
  count?: number;
}

export interface Route {
  method: string;
  path: string;
  binding: {
    componentId: {
      componentId: string;
      version: number;
    };
    workerName: string;
    response: string;
  };
}

export type HttpMethod =
  | "Get"
  | "Post"
  | "Put"
  | "Patch"
  | "Delete"
  | "Head"
  | "Options"
  | "Trace"
  | "Connect";
