import useSWR, { mutate } from "swr";
import { fetcher, getErrorMessage } from "../utils";
import { Component } from "@/types/api";
import { useEffect, useState } from "react";
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
    return { success: false, error };
  }

  mutate(ROUTE_PATH);
  mutate(`${ROUTE_PATH}/${componentId}`);
  mutate(`${ROUTE_PATH}/${componentId}/latest`);
  if (path && endpoint !== path && ROUTE_PATH !== endpoint) {
    mutate(path);
  }
  return { success: false, error: null };
}

function useComponents(componentId?: string, version?: string | number | null) {
  const [error, setError] = useState<string | null>(null);
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

  useEffect(() => {
    if (componentData) {
      const error =
        requestError ||
        (componentData?.status != 200
          ? getErrorMessage(componentData?.data)
          : null);
      setError(error);
    }
  }, [componentData]);

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

export default useComponents;
