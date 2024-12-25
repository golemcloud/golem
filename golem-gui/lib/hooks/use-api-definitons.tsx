import useSWR, { mutate } from "swr";
import { fetcher, getErrorMessage } from "../utils";
import { ApiDefinition, ApiRoute } from "@/types/api";
import { useEffect, useState } from "react";
import { useRouter } from "next/navigation";
import { toast } from "react-toastify";

const ROUTE_PATH = "?path=api/definitions";

function useApiDefinitions(defintionId?: string, version?: string | null) {
  const [error, setError] = useState<string | null>(null);
  const router = useRouter();
  defintionId = defintionId;
  let path =
    defintionId && !version
      ? `${ROUTE_PATH}?api-definition-id=${defintionId}`
      : ROUTE_PATH;
  path = defintionId && version ? `${path}/${defintionId}/${version}` : path;
  const {
    data: apiData,
    isLoading,
    error: requestError,
  } = useSWR(path, fetcher);

  const apiDefinitions = (
    defintionId && version
      ? apiData?.data
        ? [apiData?.data]
        : []
      : apiData?.data || []
  ) as ApiDefinition[];

  useEffect(() => {
    if (apiData) {
      const error =
        requestError ||
        (apiData?.status != 200 ? getErrorMessage(apiData?.data) : null);
      setError(error);
    }
  }, [apiData]);

  //if version is not given. we are providing the current working latest version routes
  const getApiDefintion = (
    id?: string,
    version?: string | null
  ): { success: boolean; error?: string | null; data?: ApiDefinition } => {
    if (!version && !id) {
      return {
        success: false,
        data: apiDefinitions[apiDefinitions.length - 1] || apiDefinitions[0],
        error: apiDefinitions.length == 0 ? "No Api defintions found!" : null,
      };
    }

    const filteredDefintions = apiDefinitions?.filter((api) => api.id === id);

    if (filteredDefintions.length === 0) {
      return { success: false, error: "No Api defintions found!" };
    }

    if (version) {
      const currentApiVersion = filteredDefintions.find(
        (api) => api.version === version
      );
      if (!currentApiVersion) {
        return {
          success: false,
          error: "No Api routes found with version given.",
        };
      }

      return { success: true, data: currentApiVersion };
    }

    return {
      success: true,
      data: filteredDefintions[filteredDefintions.length - 1],
    };
  };

  const addNewApiVersionDefinition = async (
    update: { version: string },
    id: string,
    version?: string | null
  ) => {
    const { data, error, success } = getApiDefintion(id, version);

    if (!success || !data) {
      return {
        success,
        error,
      };
    }
    //make sure new version is draft.
    const newData = { ...data, draft: true, ...update };
    const response = await fetcher(ROUTE_PATH, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
      },
      body: JSON.stringify(newData),
    });

    if (response.status !== 200) {
      const error = getErrorMessage(response.data);
      return { success: false, error };
    }
    mutate(`${ROUTE_PATH}?api-definition-id=${newData.id}`);
    mutate(`${ROUTE_PATH}/${data.id}/${update.version}`);
    mutate(`${ROUTE_PATH}`);
    if (update.version !== data.version) {
      router.push(`/apis/${newData.id}/overview?version=${newData.version}`);
    }
  };

  const deleteVersion = async (id: string, version?: string | null) => {
    try {
      const { data, error, success } = getApiDefintion(id, version);
      const noOfVersions = apiDefinitions.length;
      if (!success || !data) {
        return {
          success,
          error,
        };
      }

      const response = await fetcher(`${ROUTE_PATH}/${id}/${data.version}`, {
        method: "DELETE",
        headers: {
          "Content-Type": "application/json",
        },
      });
      if (response.status !== 200) {
        const error = getErrorMessage(response.data);
        toast.error(`Version Deletion failed. ${error}`)

        return { success: false, error };
      }

      mutate(`${ROUTE_PATH}?api-definition-id=${data.id}`);
      mutate(`${ROUTE_PATH}/${data.id}/${data.version}`);
      mutate(`${ROUTE_PATH}`);
      toast.success("Api version deleted")

      //If version we are deleting is the last version. then  redirect to api's page.
      router.push(noOfVersions > 1 ? `/apis/${id}/overview` : `/apis`);
    } catch (error) {
      console.error("Error deleting version:", error);
      toast.error(`Version Deletion failed. Something went wrong`)
      return { success: false, error: "Something went wrong" };
    }
  };

  const upsertRoute = async (
    defintionId: string,
    updateRoute: ApiRoute,
    version?: string | null
  ): Promise<{
    success: boolean;
    error?: string | null;
    data?: ApiDefinition;
  }> => {
    try {
      const { data, error, success } = getApiDefintion(defintionId, version);

      if (!success || !data) {
        return {
          success,
          error,
        };
      }
      const routes = (data?.routes || []) as ApiRoute[];
      let payload = [...(data?.routes || []), updateRoute] as ApiRoute[];
      const index = routes.findIndex(
        (route) =>
          route.path === updateRoute.path && route.method === updateRoute.method
      );
      if (index > -1) {
        routes[index] = updateRoute;
        payload = routes;
      }

      const response = await fetcher(
        `${ROUTE_PATH}/${data.id}/${data.version}`,
        {
          method: "PUT",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({
            ...data,
            routes: payload,
          }),
        }
      );

      if (response.status !== 200) {
        const error = getErrorMessage(response.data);
        toast.error(`Api definition addition/updation failed. ${error}`)

        return { success: false, error };
      }

      mutate(`${ROUTE_PATH}?api-definition-id=${data.id}`);
      mutate(`${ROUTE_PATH}/${data.id}/${data.version}`);
      mutate(`${ROUTE_PATH}`);
      toast.success("Api definition added/updated")

      return { success: true, error: null };
    } catch (error) {
      console.error("Error upserting the apidefintion:", error);
      toast.error(`Api definition addition/updation failed. Something went wrong`)

      return { success: false, error: "Something went wrong" };
    }
  };

  const deleteRoute = async (defaultRoute: ApiRoute) => {
    try {
      if (defaultRoute) {
        const { data, error } = getApiDefintion();
        if (error || !data) {
          return { success: false, error };
        }
        const routes = data.routes || [];
        const index = routes.findIndex(
          (route) =>
            route.path === defaultRoute.path &&
            route.method === defaultRoute.method
        );
        if (index == -1) {
          return { success: false, error: "No route found!" };
        }
        const newRoutes = [
          ...routes.slice(0, index),
          ...routes.slice(index + 1),
        ];
        const response = await fetcher(
          `${ROUTE_PATH}/${data.id}/${data.version}`,
          {
            method: "PUT",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({
              ...data,
              routes: newRoutes,
            }),
          }
        );
        if (response.status !== 200) {
          const error = getErrorMessage(response.data);
          toast.error(`Route deletion faile. ${error}`)

          return { success: false, error };
        }

        mutate(`${ROUTE_PATH}?api-definition-id=${data.id}`);
        mutate(`${ROUTE_PATH}/${data.id}/${data.version}`);
        mutate(`${ROUTE_PATH}`);
        toast.success("Route delete Successfully")
        return { success: true, error: null };
      }
    } catch (error) {
      console.error("Error deleting route:", error);
      toast.error(`Route deletion failed. Something went wrong`)
      return { success: false, error: "Something went wrong" };
    }
  };

  return {
    apiDefinitions,
    error,
    isLoading,
    addNewApiVersionDefinition,
    getApiDefintion,
    upsertRoute,
    deleteVersion,
    deleteRoute,
  };
}

export async function addNewApiDefinition(
  newData: ApiDefinition
): Promise<{ success: boolean; error?: string | null; data?: ApiDefinition }> {
  try {
    const response = await fetcher(ROUTE_PATH, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
      },
      body: JSON.stringify(newData),
    });

    if (response.status > 300) {
      const error = getErrorMessage(response.data);
      return { success: false, error: error };
    }

    mutate(ROUTE_PATH);
    mutate(`${ROUTE_PATH}/${newData.id}/${newData.version}`);
    mutate(`${ROUTE_PATH}/?api-definition-id=${newData.id}`);
    return { success: false, data: response.data };
  } catch (err) {
    console.log("Err", err);
    return { success: false, error: "Something went wrong!. please try again" };
  }
}

export default useApiDefinitions;
