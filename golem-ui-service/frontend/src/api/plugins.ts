import { InstalledPlugin, Plugin } from "../types/api";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { apiClient } from "../lib/api-client";

// Query keys
export const pluginKeys = {
  all: ["plugins"] as const,
  lists: () => [...pluginKeys.all, "list"] as const,
  list: (scope?: string) => [...pluginKeys.lists(), { scope }] as const,
  details: () => [...pluginKeys.all, "detail"] as const,
  detail: (name: string) => [...pluginKeys.details(), name] as const,
  version: (name: string, version: string) =>
    [...pluginKeys.detail(name), version] as const,
  installs: (componentId: string, version: number) =>
    [
      "components",
      componentId,
      "versions",
      version,
      "plugins",
      "installs",
    ] as const,
};

// API Types
export interface CreatePluginPayload {
  name: string;
  version: string;
  description: string;
  icon?: number[];
  homepage: string;
  specs: {
    type: "ComponentTransformer";
    providedWitPackage: string;
    jsonSchema: string;
    validateUrl: string;
    transformUrl: string;
  };
  scope: {
    type: "Global";
  };
}

export interface InstallPluginPayload {
  name: string;
  version: string;
  priority: number;
  parameters: Record<string, string>;
}

export interface UpdatePluginInstallPayload {
  priority: number;
  parameters: Record<string, string>;
}

// Plugin API Functions
export const getPlugins = async (scope?: string) => {
  const { data } = await apiClient.get<Plugin[]>("/v1/plugins", {
    params: { scope },
  });
  return data;
};

export const getPluginVersions = async (name: string) => {
  const { data } = await apiClient.get<Plugin[]>(`/v1/plugins/${name}`);
  return data;
};

export const getPluginVersion = async (name: string, version: string) => {
  const { data } = await apiClient.get<Plugin>(
    `/v1/plugins/${name}/${version}`,
  );
  return data;
};

export const createPlugin = async (payload: CreatePluginPayload) => {
  const { data } = await apiClient.post<void>("/v1/plugins", payload);
  return data;
};

export const deletePlugin = async (name: string, version: string) => {
  const { data } = await apiClient.delete<void>(
    `/v1/plugins/${name}/${version}`,
  );
  return data;
};

// Component Plugin Installation Functions
export const getInstalledPlugins = async (
  componentId: string,
  version: number,
) => {
  const { data } = await apiClient.get<InstalledPlugin[]>(
    `/v1/components/${componentId}/versions/${version}/plugins/installs`,
  );
  return data;
};

export const installPlugin = async (
  componentId: string,
  payload: InstallPluginPayload,
) => {
  const { data } = await apiClient.post<InstalledPlugin>(
    `/v1/components/${componentId}/latest/plugins/installs`,
    payload,
  );
  return data;
};

export const updatePluginInstallation = async (
  componentId: string,
  installationId: string,
  payload: UpdatePluginInstallPayload,
) => {
  const { data } = await apiClient.put<void>(
    `/v1/components/${componentId}/versions/latest/plugins/installs/${installationId}`,
    payload,
  );
  return data;
};

export const uninstallPlugin = async (
  componentId: string,
  installationId: string,
) => {
  const { data } = await apiClient.delete<void>(
    `/v1/components/${componentId}/latest/plugins/installs/${installationId}`,
  );
  return data;
};

// Hooks for Plugin Management
export const usePlugins = (scope?: string) => {
  return useQuery({
    queryKey: pluginKeys.list(scope),
    queryFn: () => getPlugins(scope),
  });
};

export const usePluginVersions = (name: string) => {
  return useQuery({
    queryKey: pluginKeys.detail(name),
    queryFn: () => getPluginVersions(name),
  });
};

export const usePluginVersion = (name: string, version: string) => {
  return useQuery({
    queryKey: pluginKeys.version(name, version),
    queryFn: () => getPluginVersion(name, version),
  });
}

export const useCreatePlugin = () => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: createPlugin,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: pluginKeys.lists() });
    },
  });
};

export const useDeletePlugin = () => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: ({ name, version }: { name: string; version: string }) =>
      deletePlugin(name, version),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: pluginKeys.lists() });
    },
  });
};

// Hooks for Component Plugin Installation
export const useInstalledPlugins = (componentId: string, version: number) => {
  return useQuery({
    queryKey: pluginKeys.installs(componentId, version),
    queryFn: () => getInstalledPlugins(componentId, version),
  });
};

export const useInstallPlugin = (componentId: string) => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (payload: InstallPluginPayload) =>
      installPlugin(componentId, payload),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: pluginKeys.installs(componentId, "latest"),
      });
    },
  });
};

export const useUpdatePluginInstallation = (componentId: string) => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: ({
      installationId,
      payload,
    }: {
      installationId: string;
      payload: UpdatePluginInstallPayload;
    }) => updatePluginInstallation(componentId, installationId, payload),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: pluginKeys.installs(componentId, "latest"),
      });
    },
  });
};

export const useUninstallPlugin = (componentId: string) => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (installationId: string) =>
      uninstallPlugin(componentId, installationId),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: pluginKeys.installs(componentId, "latest"),
      });
    },
  });
};
