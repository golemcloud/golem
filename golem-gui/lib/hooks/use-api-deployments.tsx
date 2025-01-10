import useSWR, { mutate } from "swr";
import { fetcher } from "../utils";
import { ApiDeployment } from "@/types/api";
import { toast } from "react-toastify";
// import { useRouter } from "next/navigation";

const ROUTE_PATH = "v1/api/deployments";

export async function addNewApiDeployment(
  newDeploy: ApiDeployment,
  path?: string
): Promise<{
  success: boolean;
  error?: string | null;
  data?: ApiDeployment | null;
}> {
  const { error, data } = await fetcher(`${ROUTE_PATH}/deploy`, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
    },
    body: JSON.stringify(newDeploy),
  });

  if (error) {
    toast.error("Failed to deploy:" + error);
    return { success: false, error };
  }
  mutate(`${ROUTE_PATH}`);
  const apiId = newDeploy?.apiDefinitions[0]?.id
  if(apiId){
    mutate(`${ROUTE_PATH}?api-definition-id=${encodeURIComponent(apiId)}`);
  }

  if (path !== ROUTE_PATH) {
    mutate(path);
  }
  toast.success("Successfully deployed");
  return { success: false, data: data };
}

function useApiDeployments(defintionId?: string, version?: string | null) {
  defintionId = defintionId;
  let path =
    defintionId && !version
      ? `${ROUTE_PATH}?api-definition-id=${encodeURIComponent(defintionId)}`
      : ROUTE_PATH;
  path = defintionId && version ? `${path}/${encodeURIComponent(defintionId)}/${encodeURIComponent(version)}` : path;
  const { data, isLoading, error } = useSWR(path, fetcher);

  const apiDeployments = (
    defintionId && version ? (data?.data ? [data?.data] : []) : data?.data || []
  ) as ApiDeployment[];

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
    const { error } = await fetcher(`${ROUTE_PATH}/${id}/${site}`, {
      method: "DELETE",
      headers: {
        "Content-Type": "application/json",
      },
    });
    if (error) {
      return { success: false, error };
    }

    mutate(`${ROUTE_PATH}`);
    if (path !== ROUTE_PATH) {
      mutate(path);
    }
  };

  return {
    apiDeployments,
    error: error || data?.error,
    isLoading,
    addApiDeployment,
    deleteDeployment,
  };
}

export default useApiDeployments;
