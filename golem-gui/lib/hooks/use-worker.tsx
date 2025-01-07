import useSWR, { mutate } from "swr";
import { fetcher } from "../utils";
import {
  Parameter,
  Worker,
  WorkerFormData,
  WorkerFunction,
  OplogQueryParams,
  Cursor,
  WorkerNormalFilter,
} from "@/types/api";
import { toast } from "react-toastify";
import { useParams, useRouter, useSearchParams } from "next/navigation";
import { useCallback, useEffect, useState } from "react";
import { WorkerFilter } from "../../types/api";
const ROUTE_PATH = "v1/components";

export function useDeleteWorker(componentId: string, workerName: string) {
  const router = useRouter();
  const endpoint = `${ROUTE_PATH}/${componentId}/workers/${workerName}`;

  const deleteWorker = async () => {
    const response = await fetcher(endpoint, { method: "DELETE" });
    if (response.error) {
      toast.success(`Fialed to  delete worker: ${response.error}`);

      return response;
    }
    toast.success("Successfully deleted the worker");
    mutate(endpoint);
    mutate(`${ROUTE_PATH}/${componentId}/workers`);
    router.push(`/components/${componentId}/workers`);
  };

  return {
    deleteWorker,
  };
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

      if (response.error) {
        toast.error("Failed to Invoked");
        return setError(response.error);
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
  data?: Worker;
}> {
  const endpoint = `${ROUTE_PATH}/${componentId}/workers`;
  const response = await fetcher(endpoint, {
    method: "POST",
    headers: {
      "content-type": "application/json",
    },
    body: JSON.stringify(newWorker),
  });

  if (response.error) {
    toast.error("Worker failed to create");
    return { success: false, error: response.error };
  }

  toast.success("Worker Sucessfully created");
  mutate(endpoint);
  mutate(`${endpoint}/find`);
  if (path && endpoint !== path) {
    mutate(path);
  }
  return { success: true, error: null, data: response.data };
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

  if (response.error) {
    toast.error(`Worker Interruption Failed due to ${response.error}`);
    return { success: false, error: response.error };
  }

  toast.success("Worker Interrupted Successfully");
  mutate(`${ROUTE_PATH}/${componentId}/workers/${workerName}`);
  return { success: true, error: null };
}

export async function resumeWorker(componentId: string, workerName: string) {
  const response = await fetcher(
    `${ROUTE_PATH}/${componentId}/workers/${workerName}/resume`,
    {
      method: "POST",
      headers: {
        "content-type": "application/json",
      },
    }
  );

  if (response.error) {
    toast.error(`Worker failed to resume due to: ${response.error}`);
    return { success: false, error: response.error };
  }

  toast.success("Worker has successfully resumed");
  mutate(`${ROUTE_PATH}/${componentId}/workers/${workerName}`);
  return { success: true, error: null };
}

export function useWorker(componentId: string, workerName: string) {
  const { data, error, isLoading } = useSWR(
    `${ROUTE_PATH}/${componentId}/workers/${workerName}`,
    fetcher
  );
  const worker = data?.data as Worker;

  return {
    error: error || data?.error,
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
    count: params.count.toString(),
    ...(params.from ? { from: params.from.toString() } : {}),
    ...(params.cursor ? { cursor: params.cursor } : {}),
    ...(params.query ? { query: params.query } : {}),
  }).toString();

  const endpoint = `${ROUTE_PATH}/${componentId}/workers/${workerName}/oplog?${queryString}`;
  const { data, error, isLoading } = useSWR(endpoint, fetcher);

  const logs = data?.data || [];

  return {
    logs,
    error: error || data?.error,
    isLoading,
  };
}

export function useWorkerFind(compId: string, limit?: number, slientToast?:boolean) {
  const [error, setError] = useState<string | null>(null);
  const [workers, setWorkers] = useState<Worker[]>([]);
  const [cursor, setCursor] = useState<Cursor>(null);
  const [isLoading, setIsLoading] = useState<boolean>(true);
  const searchParams = useSearchParams();
  const transformSearchParams = useCallback(
    (cursor?: Cursor, triggerNext?:boolean) => {
      // Parse the query string into an object
      const params = new URLSearchParams(searchParams);

      // Extract and parse workerStatus and workerVersion
      let workerStatus: string[] = [];
      let workerVersion: { version: number; comparator: string } | null = null;
      let workerName: { search: string; comparator: string } | null = null;
      let workerAfter: { type: string; value: string } | null = null;
      let workerBefore: { type: string; value: string } | null = null;
      try {
        workerStatus = JSON.parse(params.get("workerStatus") || "[]");
        workerVersion = JSON.parse(params.get("workerVersion") || "{}");
        workerName = JSON.parse(params.get("workerName") || "{}");
        workerAfter = JSON.parse(params.get("workerAfter") || "{}");
        workerBefore = JSON.parse(params.get("workerBefore") || "{}");
      } catch (e) {
        console.log("error occured while parsing", e);
      }
      // Construct filters based on workerStatus
      const statusFilters = workerStatus.map((status: string) => ({
        type: "Status",
        comparator: "Equal",
        value: status,
      }));

      // Combine the filters into the desired structure
      let defaultFilter: WorkerFilter = {
        count: limit || 10,
        precise: true,
      };
      if (cursor && triggerNext) {
        defaultFilter = {
          ...defaultFilter,
          cursor: cursor,
        };
      }

      const finalFilters = statusFilters.length
        ? {
            ...defaultFilter,
            filter: {
              type: "And",
              filters: [
                {
                  type: "Or",
                  filters: statusFilters,
                },
              ],
            },
          }
        : defaultFilter;

      // Add workerVersion if it exists
      if (
        workerVersion &&
        "version" in workerVersion &&
        workerVersion.comparator
      ) {
        finalFilters.filter = finalFilters.filter || {
          type: "And",
          filters: [],
        };
        const versionFilter: WorkerNormalFilter = {
          type: "Version",
          comparator: workerVersion.comparator,
          value: workerVersion.version,
        };
        finalFilters.filter.filters = [
          ...finalFilters.filter.filters,
          versionFilter,
        ];
      }

      if (workerName && "search" in workerName && workerName.comparator) {
        finalFilters.filter = finalFilters.filter || {
          type: "And",
          filters: [],
        };
        finalFilters.filter.filters = [
          ...finalFilters.filter.filters,
          {
            type: "Name",
            comparator: workerName.comparator,
            value: workerName.search,
          },
        ];
      }

      if (workerAfter && "value" in workerAfter) {
        finalFilters.filter = finalFilters.filter || {
          type: "And",
          filters: [],
        };
        finalFilters.filter.filters = [
          ...finalFilters.filter.filters,
          {
            type: "CreatedAt",
            comparator: "GreaterEqual",
            value: workerAfter.value,
          },
        ];
      }

      if (workerBefore && "value" in workerBefore) {
        finalFilters.filter = finalFilters.filter || {
          type: "And",
          filters: [],
        };
        finalFilters.filter.filters = [
          ...finalFilters.filter.filters,
          {
            type: "CreatedAt",
            comparator: "LessEqual",
            value: workerBefore.value,
          },
        ];
      }

      return finalFilters;
    },
    [searchParams, limit]
  );

  const triggerQuery = useCallback(
    async (cursor: Cursor, triggerNext?: boolean) => {
      const payload = transformSearchParams(cursor, triggerNext);
      try {
        setIsLoading(true);
        const response = await fetcher(`${ROUTE_PATH}/${compId}/workers/find`, {
          method: "POST",
          body: JSON.stringify(payload),
          headers: {
            "content-type": "application/json",
          },
        });

        if (response.error) {
          setError(response.error);
          if(slientToast){
            return;
          }
          return toast.error(response.error);
        }
        setWorkers((prev) => [
          ...(triggerNext ? prev : []),
          ...response.data.workers,
        ]);
        setCursor(response.data.cursor);
      } catch (err) {
        //do nothing
        console.log("error occured while fetching the data", err);
        setError("Something went wrong");
        if(slientToast){
          return;
        }
        return toast.error("Something went wrong");
      } finally {
        setIsLoading(false);
      }
    },
    [compId, slientToast, transformSearchParams]
  );

  useEffect(() => {
      triggerQuery(null)
  }, [triggerQuery]);

  return {
    error,
    data: workers,
    isLoading,
    triggerNext: cursor ? () => triggerQuery(cursor, true) : null,
    triggerQuery: triggerQuery
  };
}

export default function useWorkers(
  componentId?: string,
  version?: string | number
) {
  const { compId } = useParams<{ compId: string }>();
  const path = `${ROUTE_PATH}/${componentId || compId}/workers${
    version ? `?filter=version = ${version}` : ""
  }`;
  const { data, error, isLoading } = useSWR(path, fetcher);

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
    data?: Worker;
  }> => {
    return addNewWorker(newWorker, componentId, path);
  };

  return {
    workers,
    error: error || data?.error,
    getWorkerById,
    addWorker,
    isLoading,
  };
}

export function useWorkerFileContent(
  workersName: string,
  componentsId: string,
  fileName: string
) {
  const path = `${ROUTE_PATH}/${componentsId}/workers/${workersName}/files/${fileName}`;
  const { data, error, isLoading } = useSWR(path, fetcher);

  return { data, isLoading, error: error || data?.error };
}
