import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { ApiDefinition } from "../types/api";
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
  console.log({ id, version, definition });
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
