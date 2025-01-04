/* eslint-disable @typescript-eslint/no-explicit-any */
export interface Api   {
    createdAt?: string;
    draft: boolean;
    id: string;
    routes: any[];
    version: string;
  }

  export interface Route {
    method: string
    path: string
    binding: {
      componentId: {
        componentId: string
        version: number
      }
      workerName: string
      response: string
    }
  }
  
  