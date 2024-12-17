import {
  UseMutationOptions,
  UseQueryOptions,
  UseQueryResult,
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";

import { ApiDefinition } from "../types/api";
import { GolemError } from "../types/error";
import { apiClient } from "../lib/api-client";

// Query keys
export const apiDefinitionKeys = {
  all: ["api-definitions"] as const,
  lists: () => [...apiDefinitionKeys.all, "list"] as const,
  list: (filters: Record<string, unknown>) =>
    [...apiDefinitionKeys.lists(), filters] as const,
  details: () => [...apiDefinitionKeys.all, "detail"] as const,
  detail: (id: string, version: string) =>
    [...apiDefinitionKeys.details(), id, version] as const,
};

// API Functions
export const getApiDefinitions = async (apiDefinitionId?: string) => {
  const { data } = await apiClient.get<ApiDefinition[]>("/v1/api/definitions", {
    params: { "api-definition-id": apiDefinitionId },
  });
  return data;
};

export const getApiDefinition = async (id: string, version: string) => {
  const { data } = await apiClient.get<ApiDefinition>(
    `/v1/api/definitions/${id}/${version}`
  );
  return data;
};

export const createApiDefinition = async (
  definition: Omit<ApiDefinition, "id" | "createdAt">
) => {
  const { data } = await apiClient.post<ApiDefinition>(
    "/v1/api/definitions",
    definition
  );
  return data;
};

export const updateApiDefinition = async ({
  id,
  version,
  definition,
}: {
  id: string;
  version: string;
  definition: Partial<ApiDefinition>;
}) => {
  const { data } = await apiClient.put<ApiDefinition>(
    `/v1/api/definitions/${id}/${version}`,
    definition,
    {
      headers: { "Content-Type": "application/json" },
    }
  );
  return data;
};

export const deleteApiDefinition = async (id: string, version: string) => {
  const { data } = await apiClient.delete<string>(
    `/v1/api/definitions/${id}/${version}`
  );
  return data;
};

export const importOpenApiDefinition = async (openApiDoc: any) => {
  const { data } = await apiClient.put<ApiDefinition>(
    "/v1/api/definitions/import",
    openApiDoc
  );
  return data;
};

// Hooks
export const useApiDefinitions = (apiDefinitionId?: string) => {
  return useQuery({
    queryKey: apiDefinitionKeys.list({ apiDefinitionId }),
    queryFn: () => getApiDefinitions(apiDefinitionId),
  });
};

export const useApiDefinition = (id: string, version: string) => {
  return useQuery({
    queryKey: apiDefinitionKeys.detail(id, version),
    queryFn: () => getApiDefinition(id, version),
  });
};

export const useCreateApiDefinition = () => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: createApiDefinition,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: apiDefinitionKeys.lists() });
    },
  });
};

export const useUpdateApiDefinition = () => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: updateApiDefinition,
    onSuccess: (_, { id, version }) => {
      queryClient.invalidateQueries({
        queryKey: apiDefinitionKeys.detail(id, version),
      });
      queryClient.invalidateQueries({
        queryKey: apiDefinitionKeys.lists(),
      });
    },
  });
};

export const useDeleteApiDefinition = () => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: ({ id, version }: { id: string; version: string }) =>
      deleteApiDefinition(id, version),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: apiDefinitionKeys.lists() });
    },
  });
};

export const useImportOpenApiDefinition = () => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: importOpenApiDefinition,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: apiDefinitionKeys.lists() });
    },
  });
};
export interface ApiDeploymentInput {
  apiDefinitions: Array<{
    id: string;
    version: string;
  }>;
  site: {
    host: string;
    subdomain?: string;
  };
}

export interface ApiDeployment {
  apiDefinitions: Array<{
    id: string;
    version: string;
  }>;
  site: {
    host: string;
    subdomain?: string;
  };
  createdAt: string;
}

// Query key factory for deployments
export const deploymentKeys = {
  all: ["deployments"] as const,
  lists: () => [...deploymentKeys.all, "list"] as const,
  list: (filters: Record<string, unknown>) =>
    [...deploymentKeys.lists(), filters] as const,
  details: () => [...deploymentKeys.all, "detail"] as const,
  detail: (site: string) => [...deploymentKeys.details(), site] as const,
};

const createDeployment = async (
  deployment: ApiDeploymentInput
): Promise<ApiDeployment> => {
  const { data } = await apiClient.post<ApiDeployment>(
    "/v1/api/deployments/deploy",
    deployment
  );
  return data;
};

export const useCreateDeployment = (
  options?: UseMutationOptions<ApiDeployment, GolemError, ApiDeploymentInput>
) => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: createDeployment,
    onSuccess: (data) => {
      // Invalidate relevant queries
      queryClient.invalidateQueries({ queryKey: deploymentKeys.lists() });
      queryClient.invalidateQueries({
        queryKey: deploymentKeys.detail(data.site.host),
      });
    },
    ...options,
  });
};

// Fetch deployments for a specific API definition
const getDeployments = async (
  apiDefinitionId: string
): Promise<ApiDeployment[]> => {
  const { data } = await apiClient.get<ApiDeployment[]>("/v1/api/deployments", {
    params: {
      "api-definition-id": apiDefinitionId,
    },
  });
  return data;
};

// Fetch a single deployment by site
const getDeployment = async (site: string): Promise<ApiDeployment> => {
  const { data } = await apiClient.get<ApiDeployment>(
    `/v1/api/deployments/${site}`
  );
  return data;
};

// Hook for fetching deployments
export const useApiDeployments = (
  apiDefinitionId: string,
  options?: UseQueryOptions<ApiDeployment[], GolemError>
): UseQueryResult<ApiDeployment[], GolemError> => {
  return useQuery({
    queryKey: deploymentKeys.list({ apiDefinitionId }),
    queryFn: () => getDeployments(apiDefinitionId),
    staleTime: 30000, // Consider data fresh for 30 seconds
    ...options,
  });
};

// Hook for fetching a single deployment
export const useApiDeployment = (
  site: string,
  options?: UseQueryOptions<ApiDeployment, GolemError>
): UseQueryResult<ApiDeployment, GolemError> => {
  return useQuery({
    queryKey: deploymentKeys.detail(site),
    queryFn: () => getDeployment(site),
    staleTime: 30000, // Consider data fresh for 30 seconds
    enabled: Boolean(site), // Only run query if site is provided
    ...options,
  });
};

// Hook for fetching all deployments (with optional API definition filter)
export const useAllDeployments = (
  options?: UseQueryOptions<ApiDeployment[], GolemError>
): UseQueryResult<ApiDeployment[], GolemError> => {
  return useQuery({
    queryKey: deploymentKeys.lists(),
    queryFn: () =>
      apiClient
        .get<ApiDeployment[]>("/v1/api/deployments")
        .then((res) => res.data),
    staleTime: 30000,
    ...options,
  });
};

const deleteDeployment = async (site: string): Promise<string> => {
  const { data } = await apiClient.delete<string>(
    `/v1/api/deployments/${site}`
  );
  return data;
};

export const useDeleteDeployment = (
  options?: UseMutationOptions<string, GolemError, string>
) => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: deleteDeployment,
    onSuccess: (_, site) => {
      // Invalidate the specific deployment
      queryClient.invalidateQueries({ queryKey: deploymentKeys.details() });
      queryClient.invalidateQueries({ queryKey: deploymentKeys.list(site) });
      // queryClient.invalidateQueries({ queryKey: deploymentKeys.list({ apiDefinitionId }) });
      // Invalidate all deployment lists since they might contain this deployment
      queryClient.invalidateQueries({ queryKey: deploymentKeys.lists() });
    },
    onError: (error) => {
      console.error("Failed to delete deployment:", error);
    },
    ...options,
  });
};
