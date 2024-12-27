import useSWR, { mutate } from "swr";
import { fetcher, getErrorMessage } from "../utils";
import { ApiDeployment } from "@/types/api";
import { useMemo } from "react";
import { toast } from "react-toastify";
// import { useRouter } from "next/navigation";

const ROUTE_PATH = "?path=api/deployments";

export async function addNewApiDeployment(
  newDploy: ApiDeployment,
  path?: string
): Promise<{
  success: boolean;
  error?: string | null;
  data?: ApiDeployment | null;
}> {
  const response = await fetcher(`${ROUTE_PATH}/deploy`, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
    },
    body: JSON.stringify(newDploy),
  });

  if (response.status !== 200) {
    const error = getErrorMessage(response.data);
    toast.error("Failed to deploy:" + error)
    return { success: false, error };
  }
  mutate(`${ROUTE_PATH}`);
  if (path !== ROUTE_PATH) {
    mutate(path);
  }
  toast.success("Successfully deployed")
  return { success: false, data: response.data };
}

function useApiDeployments(defintionId?: string, version?: string | null) {
  defintionId = defintionId;
  let path =
    defintionId && !version
      ? `${ROUTE_PATH}?api-definition-id=${defintionId}`
      : ROUTE_PATH;
  path = defintionId && version ? `${path}/${defintionId}/${version}` : path;
  const { data, isLoading, error: requestError } = useSWR(path, fetcher);

  const apiDeployments = (
    defintionId && version ? (data?.data ? [data?.data] : []) : data?.data || []
  ) as ApiDeployment[];


  const error = useMemo(() => {
    if(!isLoading && data?.status!==200){
      return getErrorMessage(data);
    }
    return !isLoading ? getErrorMessage(requestError) : "";
  }, [isLoading, requestError, data]); 

  const addApiDeployment = async (
    newDeploy: ApiDeployment
  ): Promise<{
    success: boolean;
    error?: string | null;
    data?: ApiDeployment | null;
  }> => {
    return addNewApiDeployment(newDeploy, path);
  };

  //   TODO Currently we are not able to delete deployment in local.
  const deleteDeployment = async (id: string, site: string) => {
    const response = await fetcher(`${ROUTE_PATH}/${id}/${site}`, {
      method: "DELETE",
      headers: {
        "Content-Type": "application/json",
      },
    });
    if (response.status !== 200) {
      const error = getErrorMessage(response.data);
      return { success: false, error };
    }

    mutate(`${ROUTE_PATH}`);
    if (path !== ROUTE_PATH) {
      mutate(path);
    }
  };

  return {
    apiDeployments,
    error,
    isLoading,
    addApiDeployment,
    deleteDeployment,
  };
}

export default useApiDeployments;
