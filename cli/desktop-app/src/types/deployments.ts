export interface Deployment {
  apiDefinitions: string[];
  createdAt: string;
  projectId: string;
  domain: string;
  environmentId: string;
  hash: string;
  id: string;
  revision: number;
}
