import useSWR, { mutate } from "swr";
import { fetcher, getErrorMessage } from "../utils";
import {
  Component,
  InstallPluginPayload,
  UpdatePluginInstallPayload,
} from "@/types/api";
import { toast } from "react-toastify";
import { useMemo } from "react";
// import { useRouter } from "next/navigation";

const ROUTE_PATH = "?path=components";

export async function getLatestComponent(componentId: string) {
  const response = await fetcher(`${ROUTE_PATH}/${componentId}/latest`, {
    method: "GET",
  });

  if (response.status !== 200) {
    const error = getErrorMessage(response.data);
    return { success: false, error };
  }
  return { success: false, error: null };
}

export async function addNewcomponent(
  update: FormData,
  componentId?: string,
  mode = "create",
  path?: string
): Promise<{
  success: boolean;
  error?: string | null;
  data?: Component;
}> {
  const endpoint =
    mode === "create" ? ROUTE_PATH : `${ROUTE_PATH}/${componentId}/updates`;
  const response = await fetcher(endpoint, {
    method: "POST",
    body: update,
  });

  if (response.status !== 200) {
    const error = getErrorMessage(response.data);
    toast.success(
      `Component Failed to ${mode === "create" ? "create" : "update"}`
    );

    return { success: false, error };
  }

  mutate(ROUTE_PATH);
  mutate(`${ROUTE_PATH}/${componentId}`);
  mutate(`${ROUTE_PATH}/${componentId}/latest`);
  toast.success(
    `Component Successfully ${mode === "create" ? "Created" : "Updated"}`
  );
  if (path && endpoint !== path && ROUTE_PATH !== endpoint) {
    mutate(path);
  }
  return { success: true, error: null };
}

function useComponents(componentId?: string, version?: string | number | null) {
  //   const router = useRouter();
  componentId = componentId;
  let path =
    componentId && !version ? `${ROUTE_PATH}/${componentId}` : ROUTE_PATH;
  path =
    componentId && version
      ? `${path}/${componentId}/${
          version === "latest" ? version : `versions/${version}`
        }`
      : path;
  const {
    data: componentData,
    isLoading,
    error: requestError,
  } = useSWR(path, fetcher);

  const components = (
    componentId && version
      ? componentData?.data
        ? [componentData?.data]
        : []
      : componentData?.data || []
  ) as Component[];

  const error = useMemo(() => {
    if (!isLoading && componentData?.status !== 200) {
      return getErrorMessage(componentData);
    }
    return !isLoading ? getErrorMessage(requestError) : "";
  }, [isLoading, requestError, componentData]);

  const getComponent = (
    id?: string,
    version?: string | number | null
  ): { success: boolean; error?: string | null; data?: Component } => {
    if (!version && version !== 0 && !id) {
      return {
        success: false,
        data: components[components.length - 1] || components[0],
        error: components.length == 0 ? "No component components found!" : null,
      };
    }

    const filteredcomponents = components?.filter(
      (component: Component) =>
        component.versionedComponentId.componentId === id
    );

    if (filteredcomponents.length === 0) {
      return { success: false, error: "No component components found!" };
    }

    if (version) {
      const currentcomponentVersion = filteredcomponents.find(
        (component: Component) =>
          component.versionedComponentId.version == version
      );
      if (!currentcomponentVersion) {
        return {
          success: false,
          error: "No component routes found with version given.",
        };
      }

      return { success: true, data: currentcomponentVersion };
    }

    return {
      success: true,
      data: filteredcomponents[filteredcomponents.length - 1],
    };
  };

  const upsertComponent = async (
    componentId: string,
    update: FormData,
    mode = "create"
  ): Promise<{
    success: boolean;
    error?: string | null;
    data?: Component;
  }> => {
    return addNewcomponent(update, componentId, mode, path);
  };

  return {
    components,
    error,
    isLoading,
    upsertComponent,
    getComponent,
  };
}

export async function installPlugin(
  payload: InstallPluginPayload,
  componentId: string,
  version?: number | string
) {
  let endpoint = `${ROUTE_PATH}/${componentId}`;
  endpoint =
    typeof version == "number" || version
      ? `${endpoint}/${version}`
      : `${endpoint}/latest`;

  const response = await fetcher(endpoint, {
    method: "PUT",
    headers: {
      "content-type": "application/json",
    },
    body: JSON.stringify(payload),
  });

  if (response.status !== 200) {
    const error = getErrorMessage(response.data);
    toast.success(`Plugin Failed to Install: ${error}`);
    return { success: false, error };
  }

  mutate(ROUTE_PATH);
  mutate(endpoint);
  toast.success(`Plugin successfully installed`);
  return { success: true, error: null };
}

export function useInstallPlugins(
  componentId: string,
  version?: number | string
) {
  let endpoint = `${ROUTE_PATH}/${componentId}`;
  endpoint =
    typeof version == "number" || version
      ? `${endpoint}/${version}`
      : `${endpoint}/latest`;

  const { data, error: requestError, isLoading } = useSWR(endpoint, fetcher);

  const installedPlugins = data?.data || [];

  const error = useMemo(() => {
    if (!isLoading && data?.status !== 200) {
      return getErrorMessage(data);
    }
    return !isLoading ? getErrorMessage(requestError) : "";
  }, [isLoading, requestError, data]);

  return {
    installedPlugins,
    isLoading,
    error,
  };
}

export function useUninstallPlugin(
  componentId: string,
  version?: string | number
) {
  let endpoint = `${ROUTE_PATH}/${componentId}`;
  endpoint =
    typeof version == "number" || version
      ? `${endpoint}/${version}`
      : `${endpoint}/latest`;

  const uninstallPlugin = async (installationId: string) => {
    const response = await fetcher(`${endpoint}/${installationId}`, {
      method: "DLETE",
    });

    if (response.status !== 200) {
      const error = getErrorMessage(response.data);
      toast.success(`Plugin Failed to Uninstall: ${error}`);
      return { success: false, error };
    }

    mutate(ROUTE_PATH);
    mutate(endpoint);
    toast.success(`Plugin successfully uninstalled`);
    return { success: true, error: null };
  };

  return {
    uninstallPlugin,
  };
}

export function useUpdateInstallPlugin(
  componentId: string,
  version?: number | string
) {
  let endpoint = `${ROUTE_PATH}/${componentId}`;
  endpoint =
    typeof version == "number" || version
      ? `${endpoint}/${version}`
      : `${endpoint}/latest`;

  const updateInstalledPlugin = async (payload: UpdatePluginInstallPayload) => {
    const response = await fetcher(endpoint, {
      method: "PUT",
      headers: {
        "content-type": "application/json",
      },
      body: JSON.stringify(payload),
    });

    if (response.status !== 200) {
      const error = getErrorMessage(response.data);
      toast.success(`Plugin Failed to update: ${error}`);
      return { success: false, error };
    }

    mutate(ROUTE_PATH);
    mutate(endpoint);
    toast.success(`Plugin successfully updated`);
    return { success: true, error: null };
  };
  return {
    updateInstalledPlugin,
  };
}

export default useComponents;
