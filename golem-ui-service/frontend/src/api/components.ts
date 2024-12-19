import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { Component } from "../types/api";
import { apiClient } from "../lib/api-client";

// Query keys
export const componentKeys = {
  all: ["components"] as const,
  lists: () => [...componentKeys.all, "list"] as const,
  list: (filters: Record<string, unknown>) =>
    [...componentKeys.lists(), filters] as const,
  details: () => [...componentKeys.all, "detail"] as const,
  detail: (id: string) => [...componentKeys.details(), id] as const,
  versions: (id: string) => [...componentKeys.detail(id), "versions"] as const,
};

// API functions
export const getComponents = async (componentName?: string) => {
  const { data } = await apiClient.get<Component[]>("/v1/components", {
    params: { "component-name": componentName },
  });
  return data;
};

export const getComponentVersions = async (componentId: string) => {
  const { data } = await apiClient.get<Component[]>(
    `/v1/components/${componentId}`,
  );
  return data;
};

export const deleteComponent = async (componentId: string) => {
  const { data } = await apiClient.delete(`/v1/components/${componentId}`);
  return data;
};

export const getComponentVersion = async (
  componentId: string,
  version: number,
) => {
  const { data } = await apiClient.get<Component>(
    `/v1/components/${componentId}/versions/${version}`,
  );
  return data;
};

export const createComponent = async (formData: FormData) => {
  const { data } = await apiClient.post<Component>("/v1/components", formData, {
    headers: {
      "Content-Type": "multipart/form-data",
    },
  });
  return data;
};

export const updateComponent = async ({
  componentId,
  formData,
}: {
  componentId: string;
  formData: FormData;
}) => {
  const { data } = await apiClient.post<Component>(
    `/v1/components/${componentId}/updates`,
    formData,
    {
      headers: {
        "Content-Type": "multipart/form-data",
      },
    },
  );
  return data;
};

// Hooks
export const useComponents = (
  componentName?: string,
): {
  data: Component[];
  isLoading: boolean;
} => {
  return useQuery({
    queryKey: componentKeys.list({ componentName }),
    queryFn: () => getComponents(componentName),
  });
};

export const useComponentVersions = (componentId: string) => {
  return useQuery({
    queryKey: componentKeys.versions(componentId),
    queryFn: () => getComponentVersions(componentId),
  });
};

export const useCreateComponent = () => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: createComponent,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: componentKeys.lists() });
    },
  });
};

export const useUpdateComponent = () => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: updateComponent,
    onSuccess: (_, { componentId }) => {
      queryClient.invalidateQueries({
        queryKey: componentKeys.detail(componentId),
      });
    },
  });
};
export const useDeleteComponent = () => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (params: { id: string }) => deleteComponent(params.id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: componentKeys.lists() });
    },
  });
};

export const useComponent = (componentId: string) => {
  return useQuery({
    queryKey: componentKeys.detail(componentId),
    queryFn: () => getComponent(componentId),
    enabled: !!componentId, // Only run if componentId is provided
  });
};

export const getComponent = async (componentId: string) => {
  const { data } = await apiClient.get<Component>(
    `/v1/components/${componentId}/latest`,
  );
  return data;
};
