import useSWR, { mutate } from "swr";
import { fetcher, getErrorMessage } from "../utils";
import {
  Component,
  Parameter,
  Worker,
  WorkerFormData,
  WorkerFunction,
  OplogQueryParams
} from "@/types/api";
import { useParams, useRouter } from "next/navigation";
import { useEffect, useMemo, useState } from "react";
import { toast } from "react-toastify";
const ROUTE_PATH = "?path=components";

export function useDeleteWorker(componentId: string, workerName: string) {
  const router = useRouter();
  const endpoint = `${ROUTE_PATH}/${componentId}/workers/${workerName}`;

  const deleteWorker = async()=>{
  const response = await fetcher(endpoint, { method: "DELETE" });
  if (response.status !== 200) {
    const error = getErrorMessage(response.data);
    toast.success(`Fialed to  delete worker: ${error}`)

    return { success: false, error };
  }
  toast.success("Successfully deleted the worker")
  mutate(endpoint);
  mutate(`${ROUTE_PATH}/${componentId}/workers`);
  router.push(`/components/${componentId}/workers`);
  }

  return {
    deleteWorker
  }
} 

export function getStateFromWorkersData(workers: Worker[]) {
  if (!workers) {
    return {};
  }
  return workers.reduce<Record<string, number>>((obj, worker) => {
    const key = worker?.status?.toLowerCase();
    if (key) {
      // Ensure `worker.status` exists
      obj[key] = (obj[key] || 0) + 1;
    }
    return obj;
  }, {});
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function transform(inputs: Parameter[], data: Record<string, any>) {
  return inputs?.map((input) => {
    const { name } = input;

    let value = null;

    if (name in data) {
      // Use provided data value
      value = data[name];
    }

    return { ...input, value };
  });
}

export function useWorkerInvocation(invoke: {
  fun?: WorkerFunction;
  instanceName?: string | null;
}) {
  const { compId, id: workerName } = useParams<{
    compId: string;
    id: string;
  }>();

  const instanceName = invoke?.instanceName;
  const functionName = invoke?.fun?.name;

  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState(null);

  useEffect(() => {
    setError(null);
    setResult(null);
  }, [invoke]);

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const invokeFunction = async (data: any) => {
    try {
      const payload = transform(invoke?.fun?.parameters || [], data);
      let endpoint = `${ROUTE_PATH}/${compId}/workers/${workerName}/invoke-and-await?function=`;
      endpoint =
        instanceName && functionName
          ? `${endpoint}${instanceName}.{${functionName}}`
          : `${endpoint}${functionName}`;
      const response = await fetcher(endpoint, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ params: payload }),
      });

      if (response.status !== 200) {
        toast.error("Failed to Invoked");
        return setError(getErrorMessage(response.data));
      }
      setError(null);
      setResult(response.data);
      toast.success("Successfully Invoked");
      mutate(`${ROUTE_PATH}/${compId}/workers/${workerName}`);
      mutate(`${ROUTE_PATH}/${compId}/workers`);
    } catch (err) {
      console.log("error", err);
      setError("Something went wrong. try again");
    }
  };

  return {
    result,
    error,
    invokeFunction,
  };
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
      "content-type": "application/json",
    },
    body: JSON.stringify(newWorker),
  });

  if (response.status !== 200) {
    const error = getErrorMessage(response.data);
    toast.error("Worker failed to create");
    return { success: false, error };
  }

  toast.success("Worker Sucessfully created");
  mutate(endpoint);
  if (path && endpoint !== path) {
    mutate(path);
  }
  return { success: false, error: null };
}

export async function interruptWorker(
  componentId: string,
  workerName: string,
  recover?: boolean
) {
  const response = await fetcher(
    `${ROUTE_PATH}/${componentId}/workers/${workerName}/interrupt${
      typeof recover === "boolean" ? `recovery-immediately=${recover}` : ""
    }`,
    {
      method: "POST",
      headers: {
        "content-type": "application/json",
      },
    }
  );

  if (response.status !== 200) {
    const error = getErrorMessage(response.data);
    toast.error(`Worker Interruption Failed due to ${error}`);
    return { success: false, error };
  }

  toast.success("Worker Interrupted Successfully");
  mutate(`${ROUTE_PATH}/${componentId}/workers/${workerName}`)
  return {success: true, error: null}
}

export async function resumeWorker(
  componentId: string,
  workerName: string,
) {
  const response = await fetcher(
    `${ROUTE_PATH}/${componentId}/workers/${workerName}/resume`,
    {
      method: "POST",
      headers: {
        "content-type": "application/json",
      },
    }
  );

  if (response.status !== 200) {
    const error = getErrorMessage(response.data);
    toast.error(`Worker failed to resume due to: ${error}`);
    return { success: false, error };
  }

  toast.success("Worker has successfully resumed");
  mutate(`${ROUTE_PATH}/${componentId}/workers/${workerName}`)
  return {success: true, error: null}
}

export function useWorker(componentId: string, workerName: string) {
  const {
    data,
    error: requestError,
    isLoading,
  } = useSWR(`${ROUTE_PATH}/${componentId}/workers/${workerName}`, fetcher);

  const error = useMemo(() => {
    if (!isLoading && data?.status !== 200) {
      return getErrorMessage(data);
    }
    return !isLoading ? getErrorMessage(requestError) : "";
  }, [isLoading, requestError, data]);
  const worker = data?.data as Worker;

  return {
    error,
    worker,
    isLoading,
  };
}

export function useWorkerLogs(
  componentId: string,
  workerName: string,
  params: OplogQueryParams
) {
  const queryString = new URLSearchParams({
    ...(params.from ? { from: params.from.toString() } : {}),
    ...(params.cursor ? { cursor: params.cursor } : {}),
    ...(params.query ? { query: params.query } : {}),
    count: params.count.toString(),
  }).toString();

  const endpoint = `${ROUTE_PATH}/${componentId}/workers/${workerName}/oplog?${queryString}`;
  const {
    data,
    error: requestError,
    isLoading,
  } = useSWR(endpoint, fetcher);

  const error =
    requestError || (data && data?.status !== 200)
      ? getErrorMessage(data?.data)
      : "";
  const logs = data?.data || [];
  
  return {
    logs,
    error,
    isLoading,
  };
}

export default function useWorkers(componentId?: string, version?: string | number) {
  const { compId } = useParams<{ compId: string }>();
  const path = `${ROUTE_PATH}/${componentId || compId}/workers${
    version ? `?filter=version = ${version}` : ""
  }`;
  const { data, error: requestError, isLoading } = useSWR(path, fetcher);

  const error = useMemo(() => {
    if (!isLoading && data?.status !== 200) {
      return getErrorMessage(data);
    }
    return !isLoading ? getErrorMessage(requestError) : "";
  }, [isLoading, requestError, data]);
  const workers = (data?.data?.workers || []) as Worker[];

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
    newWorker: WorkerFormData
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
    isLoading,
  };
}

export function WorkerFileContent(workersName: string,componentsId:string, fileName: string){
  const path = `${ROUTE_PATH}/${componentsId}/workers/${workersName}/files/${fileName}`
  console.log("path", path);
  const { data, error: requestError, isLoading } = useSWR(path, fetcher);

  const error =
    requestError || (data && data?.status != 200)
      ? getErrorMessage(data?.data)
      : "";
  if(error) return error;
  return {data, isLoading};
}

