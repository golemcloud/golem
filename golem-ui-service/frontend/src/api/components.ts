import {
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";

import { Component } from "../types/api";
import { GolemError } from "../types/error";
import { apiClient } from "../lib/api-client";
import { displayError } from "../lib/error-utils";

// Query keys
export const componentKeys = {
  all: ["components"] as const,
  lists: () => [...componentKeys.all, "list"] as const,
  list: (filters: Record<string, unknown>) =>
    [...componentKeys.lists(), filters] as const,
  details: () => [...componentKeys.all, "detail"] as const,
  detail: (id: string, version: string | number) =>
    [...componentKeys.details(), id, version] as const,
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
    `/v1/components/${componentId}`
  );
  return data;
};

export const deleteComponent = async (componentId: string) => {
  const { data } = await apiClient.delete(`/v1/components/${componentId}`);
  return data;
};

export const getComponentVersion = async (
  componentId: string,
  version: string | number
) => {
  const { data } = await apiClient.get<Component>(
    `/v1/components/${componentId}/versions/${version}`
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
    }
  );
  return data;
};

// Hooks
export const useComponents = (
  componentName?: string
): {
  data: Component[];
  isLoading: boolean;
} => {
  return useQuery({
    queryKey: componentKeys.list({ componentName }),
    queryFn: () => getComponents(componentName),
    onError: (error: Error | GolemError) =>
      displayError(error, "Error fetching components"),
  });
};

export const useComponentVersions = (componentId: string) => {
  return useQuery({
    queryKey: componentKeys.versions(componentId),
    queryFn: () => getComponentVersions(componentId),
    onError: (error: Error | GolemError) =>
      displayError(error, "Error fetching component versions"),
  });
};

export const useCreateComponent = () => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: createComponent,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: componentKeys.lists() });
    },
    onError: (error: Error | GolemError) =>
      displayError(error, "Error creating component"),
  });
};

export const useUpdateComponent = () => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: updateComponent,
    onSuccess: (_, { componentId }) => {
      // queryClient.invalidateQueries({
      //   queryKey: componentKeys.detail(componentId),
      // });
    },
    onError: (error: Error | GolemError) =>
      displayError(error, "Error updating component"),
  });
};
export const useDeleteComponent = () => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (params: { id: string }) => deleteComponent(params.id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: componentKeys.lists() });
    },
    onError: (error: Error | GolemError) =>
      displayError(error, "Error deleting component"),
  });
};

export const useComponent = (
  componentId: string,
  version: string | number
): {
  data: Component;
  isLoading: boolean;
} => {
  return useQuery({
    queryKey: componentKeys.detail(componentId, version),
    queryFn: () => getComponentVersion(componentId, version),
    onError: (error: Error | GolemError) =>
      displayError(error, "Error fetching component"),
    enabled: !!componentId && !!version, // Only run if componentId is provided
    cacheTime: 0, // Disable cache
  });
};

export const getComponent = async (componentId: string) => {
  const { data } = await apiClient.get<Component>(
    `/v1/components/${componentId}/latest`
  );
  return data;
};
