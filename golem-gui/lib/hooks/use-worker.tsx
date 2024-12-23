import useSWR, { mutate } from "swr";
import { fetcher, getErrorMessage } from "../utils";
import { Component, Worker, WorkerFormData } from "@/types/api";
import { useParams } from "next/navigation";
// import { useRouter } from "next/navigation";
const ROUTE_PATH = "?path=components";

export async function deleteWorker(componentId:string, workerName:string){
  const endpoint = `${ROUTE_PATH}/${componentId}/workers/${workerName}`;
  const response = await fetcher(endpoint, {method: "DELETE"});
  if (response.status !== 200) {
    const error = getErrorMessage(response.data);
    return { success: false, error };
  }
  mutate(endpoint);
  mutate(`${ROUTE_PATH}/${componentId}/workers`);
}

export function getStateFromWorkersData(workers: Worker[]){
  if(!workers){
    return {}
  }
  console.log("workers---<", workers)
  return workers.reduce<Record<string, number>>((obj, worker) => {
    const key = worker?.status?.toLowerCase()
    if (key) { // Ensure `worker.status` exists
      obj[key] = (obj[key] || 0) + 1;
    }
    return obj;
  }, {});
}

export async function addNewWorker(
  newWorker: WorkerFormData,
  componentId: string,
  path?: string
): Promise<{
  success: boolean;
  error?: string | null;
  data?: Component;
}> {
  const endpoint = `${ROUTE_PATH}/${componentId}/workers`;
  const response = await fetcher(endpoint, {
    method: "POST",
    headers: {
      "content-type": "application/json"
    },
    body: JSON.stringify(newWorker),
  });

  if (response.status !== 200) {
    const error = getErrorMessage(response.data);
    return { success: false, error };
  }


  mutate(endpoint);
  if (path && endpoint !== path) {
    mutate(path);
  }
  return { success: false, error: null };
}


function useWorkers(
  componentId?: string,
  version?: string | number,
) {
  const { compId } = useParams<{ compId: string }>();
  const path = `${ROUTE_PATH}/${componentId || compId}/workers${version? `?filter=version = ${version}`: ''}`;
  const {data, error: requestError, isLoading} = useSWR(path, fetcher);

  const error = requestError || (data && data?.status!=200) ? getErrorMessage(data?.data) : ""
  const workers = (data?.data?.workers || []) as Worker[]
  
  const getWorkerById = (
    id: string
  ): { success: boolean; error?: string | null; data?: Worker } => {
    const worker = workers?.find(
      (worker: Worker) => worker.workerId.workerName === id
    );

    if (!worker) {
      return { success: false, error: "No component components found!" };
    }

    return {
      success: true,
      data: worker,
    };
  };

  const addWorker = async (
    componentId: string,
    newWorker: WorkerFormData,
  ): Promise<{
    success: boolean;
    error?: string | null;
    data?: Component;
  }> => {
    return addNewWorker(newWorker, componentId, path);
  };

  return {
    workers,
    error,
    getWorkerById,
    addWorker,
    isLoading
  };
}

export default useWorkers;
