import useSWR, { mutate } from "swr";
import { fetcher, getErrorMessage } from "../utils";
import { ApiDefinition, ApiRoute } from "@/types/api";
import { useRouter } from "next/navigation";
import { toast } from "react-toastify";

const ROUTE_PATH = "v1/api/definitions";

function useApiDefinitions(defintionId?: string, version?: string | null) {
  const router = useRouter();
  defintionId = defintionId;
  let path =
    defintionId && !version
      ? `${ROUTE_PATH}?api-definition-id=${encodeURIComponent(defintionId)}`
      : ROUTE_PATH;
  path = defintionId && version ? `${path}/${encodeURIComponent(defintionId)}/${encodeURIComponent(version)}` : path;
  const { data: apiData, isLoading, error } = useSWR(path, fetcher);

  const apiDefinitions = (
    defintionId && version
      ? apiData?.data
        ? [apiData?.data]
        : []
      : apiData?.data || []
  ) as ApiDefinition[];

  //if version is not given. we are providing the current working latest version routes
  const getApiDefintion = (
    id?: string | null,
    version?: string | null
  ): { success: boolean; error?: string | null; data?: ApiDefinition } => {
    if(isLoading){
      return {success: false};
    }

    if (!version && !id) {
      return {
        success: true,
        data: apiDefinitions[apiDefinitions.length - 1] || apiDefinitions[0],
        error: apiDefinitions.length == 0 ? "No Api defintions found!" : null,
      };
    }
    const filteredDefintions = (id || defintionId) ? apiDefinitions?.filter((api) => (api.id === id) ||(api.id === defintionId)): [];

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
    version?: string | null,
    noRedirect?: boolean | undefined
  ) => {
    const { data, error, success } = getApiDefintion(id, version);

    if (!success || !data) {
      return {
        success,
        error: error || apiData?.error,
      };
    }
    //make sure new version is draft.
    const newData = { ...data, draft: true, ...update };
    const { error: requestError } = await fetcher(ROUTE_PATH, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
      },
      body: JSON.stringify(newData),
    });

    if (requestError) {
      toast.error(requestError);
      return { success: false, error: requestError };
    }
    toast.success("Successfully created new version.");
    mutate(`${ROUTE_PATH}?api-definition-id=${encodeURIComponent(newData.id)}`);
    mutate(`${ROUTE_PATH}/${encodeURIComponent(data.id)}/${encodeURIComponent(update.version)}`);
    mutate(`${ROUTE_PATH}`);
    if (!noRedirect && update.version !== data.version) {
      router.push(`/apis/${newData.id}/overview?version=${newData.version}`);
    }
  };

  const deleteVersion = async (id: string, version?: string | null, noRedirect?:boolean) => {
    try {
      const { data, error, success } = getApiDefintion(id, version);
      const noOfVersions = apiDefinitions.length;
      if (!success || !data) {
        toast.error(error);
        return {
          success,
          error,
        };
      }

      const { error: requestError } = await fetcher(
        `${ROUTE_PATH}/${id}/${data.version}`,
        {
          method: "DELETE",
          headers: {
            "Content-Type": "application/json",
          },
        }
      );
      if (requestError) {
        toast.error(`Version Deletion failed. ${requestError}`);

        return { success: false, error: requestError };
      }

      mutate(`${ROUTE_PATH}?api-definition-id=${encodeURIComponent(data.id)}`);
      mutate(`${ROUTE_PATH}/${encodeURIComponent(data.id)}/${encodeURIComponent(data.version)}`);
      mutate(`${ROUTE_PATH}`);
      toast.success("Api version deleted");

      //If version we are deleting is the last version. then  redirect to api's page.
      if(!noRedirect){
        router.push(noOfVersions > 1 ? `/apis/${id}/overview` : `/apis`);
      }
    } catch (error) {
      console.error("Error deleting version:", error);
      toast.error(`Version Deletion failed. Something went wrong`);
      return { success: false, error: "Something went wrong" };
    }
  };

  const upsertRoute = async (
    defintionId: string,
    updateRoute: ApiRoute,
    version?: string | null,
    routePath? : string|null
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
          route.path === routePath && route.method === updateRoute.method
      );
      if (index > -1 && routePath) {
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

      if (response.error) {
        toast.error(
          `Api definition addition/updation failed. ${response.error}`
        );
        return response;
      }

      mutate(`${ROUTE_PATH}?api-definition-id=${encodeURIComponent(data.id)}`);
      mutate(`${ROUTE_PATH}/${encodeURIComponent(data.id)}/${encodeURIComponent(data.version)}`);
      mutate(`${ROUTE_PATH}`);
      toast.success("Api definition added/updated");
      return response;
    } catch (error) {
      console.error("Error upserting the apidefintion:", error);
      toast.error(
        `Api definition addition/updation failed. Something went wrong`
      );

      return { success: false, error: "Something went wrong" };
    }
  };

 
  const deleteRoute = async (defaultRoute: ApiRoute, version?: string|null) => {
    try {
      if (defaultRoute) {
        const { data, error } = getApiDefintion(null, version);
        if (error || !data) {
          return { success: false, error };
        }
        const routes = data.routes || [];

        console.log("routes======>", data);
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
        if (response.error) {
          const error = getErrorMessage(response.data);
          toast.error(`Route deletion failed. ${error}`);
          return response;
        }

        mutate(`${ROUTE_PATH}?api-definition-id=${encodeURIComponent(data.id)}`)
        mutate(`${ROUTE_PATH}/${encodeURIComponent(data.id)}/${encodeURIComponent(data.version)}`);
        mutate(`${ROUTE_PATH}`);
        toast.success("Route deleted Successfully");
        return response;
      }else {
        toast.error("No route Found!")
      }
    } catch (error) {
      console.error("Error deleting route:", error);
      toast.error(`Route deletion failed. Something went wrong`);
      return { success: false, error: "Something went wrong" };
    }
  };

  return {
    apiDefinitions,
    error: error || apiData?.error,
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

    if (response.error) {
      const error = getErrorMessage(response.data);
      toast.error(`Api creation/updation failed. ${error}`);
      return response;
    }

    mutate(ROUTE_PATH);
    mutate(`${ROUTE_PATH}/${encodeURIComponent(newData.id)}/${encodeURIComponent(newData.version)}`);
    mutate(`${ROUTE_PATH}/?api-definition-id=${encodeURIComponent(newData.id)}`);
    toast.success(`Api successfully created/updated`);
    return response;
  } catch (err) {
    console.log("Err", err);
    toast.error(`Something went wrong!`);
    return { success: false, error: "Something went wrong!. please try again" };
  }
}

export const downloadApi = async(apiId:string, version?:string)=>{

    try{
        const {data:apiDefinition,error} = await fetcher(`${ROUTE_PATH}${version ? `/${encodeURIComponent(apiId)}/${encodeURIComponent(version)}`: `?api-definition-id=${encodeURIComponent(apiId)}`}`);
        
        const api = Array.isArray(apiDefinition)? apiDefinition[apiDefinition.length-1]: apiDefinition
        if(!api || error){
          return toast.error(`Failed to downalod: ${error || 'No api found!'}`)
        }
        const jsonString = JSON.stringify(api, null, 2); // Pretty print with 2 spaces
        const blob = new Blob([jsonString], { type: "application/json" });
        const url = URL.createObjectURL(blob);
        const link = document.createElement("a");
        link.href = url;
        link.download = `${api.id}-${api.version}.json`; // The name of the file to download
    
        // Trigger the download
        document.body.appendChild(link);
        link.click();
    
        // Clean up and remove the link
        link.remove();
        URL.revokeObjectURL(url);
        return toast.success("Successfully triggered");
    }catch(err){
      console.error("error occurred while downlaoding the api", err);
      toast.error("Something went wrong!")
    }
  }


export default useApiDefinitions;
