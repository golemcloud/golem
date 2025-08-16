export interface Deployment {
  apiDefinitions: {
    id: string;
    version: string;
  }[];
  createdAt: string;
  projectId: string;
  site: {
    host: string;
    subdomain: string | null;
  };
}
