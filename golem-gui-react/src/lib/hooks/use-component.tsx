import useSWR, { mutate } from "swr";
import { fetcher, getErrorMessage } from "../utils";
import {
  Component,
  InstallPluginPayload,
  UpdatePluginInstallPayload,
} from "@lib/types/api";
import { toast } from "react-toastify";
import {writeFile} from "@tauri-apps/plugin-fs";
import {BaseDirectory} from "@tauri-apps/api/path";

const ROUTE_PATH = "v1/components";

export async function getLatestComponent(componentId: string) {
  const response = await fetcher(`${ROUTE_PATH}/${componentId}/latest`, {
    method: "GET",
  });

  if (response.error) {
    return response;
  }
  return response;
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

  if (response.error) {
    toast.error(
      `Component Failed to ${mode === "create" ? "create" : "update"}`
    );
    return response;
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
  const { data: componentData, isLoading, error } = useSWR(path, fetcher);

  const components = (
    componentId && version
      ? componentData?.data
        ? [componentData?.data]
        : []
      : componentData?.data || []
  )?.sort((comp1:Component, comp2:Component)=> comp1.versionedComponentId.version-comp2.versionedComponentId.version) as Component[];

  const getComponent = (
    id?: string,
    version?: string | number | null
  ): { success: boolean; error?: string | null; data?: Component } => {
    if (!version && version !== 0 && !id) {
      return {
        success: components.length == 0,
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
    error: error || componentData?.error,
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

  if (response.error) {
    toast.success(`Plugin Failed to Install: ${response.error}`);
    return response;
  }

  mutate(ROUTE_PATH);
  mutate(endpoint);
  toast.success(`Plugin successfully installed`);
  return response;
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

  const { data, error, isLoading } = useSWR(endpoint, fetcher);

  const installedPlugins = data?.data || [];

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

    if (response.error) {
      toast.success(`Plugin Failed to Uninstall: ${response.error}`);
      return response;
    }

    mutate(ROUTE_PATH);
    mutate(endpoint);
    toast.success(`Plugin successfully uninstalled`);
    return response;
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

    if (response.error) {
      const error = getErrorMessage(response.data);
      toast.success(`Plugin Failed to update: ${error}`);
      return response;
    }

    mutate(ROUTE_PATH);
    mutate(endpoint);
    toast.success(`Plugin successfully updated`);
    return response;
  };
  return {
    updateInstalledPlugin,
  };
}

  
export async function downloadComponent(compId: string, version: number | string) {
  try {
    const url = `${ROUTE_PATH}/${compId}/download${version ? `?version=${version}` : ""}`;
    console.log("Downloading from:", url); // Debugging

    const response = await fetch(url);

    if (!response.ok) {
      throw new Error(`Failed to fetch: ${response.statusText}`);
    }

    // Ensure binary response
    const arrayBuffer = await response.arrayBuffer();
    const fileData = new Uint8Array(arrayBuffer);

    if (fileData.length === 0) {
      throw new Error("Downloaded file is empty.");
    }

    const fileName = `${compId}-${version}.wasm`;

    // Write file to Downloads directory
    await writeFile(fileName, fileData, {
      baseDir: BaseDirectory.Download,
    });

    return toast.success("Successfully downloaded");
  } catch (err) {
    console.error("Error occurred while downloading the component:", err);
    toast.error("Something went wrong");
  }
}

export default useComponents;
