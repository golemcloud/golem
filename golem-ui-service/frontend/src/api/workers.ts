import {
  UseMutationOptions,
  UseMutationResult,
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { Worker, WorkerStatus } from "../types/api";

import { GolemError } from "../types/error";
import { apiClient } from "../lib/api-client";
import { displayError } from "../lib/error-utils";

// Query keys
export const workerKeys = {
  all: ["workers"] as const,
  lists: () => [...workerKeys.all, "list"] as const,
  list: (componentId: string, filters: Record<string, unknown>) =>
    [...workerKeys.lists(), componentId, filters] as const,
  details: () => [...workerKeys.all, "detail"] as const,
  detail: (componentId: string, workerName: string) =>
    [...workerKeys.details(), componentId, workerName] as const,
  files: (componentId: string, workerName: string) =>
    [...workerKeys.detail(componentId, workerName), "files"] as const,
};

interface WorkerFilter {
  type: "Name" | "Version" | "Status" | "Env" | "CreatedAt";
  comparator: string;
  value: string | number | WorkerStatus;
  name?: string; // For Env filter
}

interface WorkerListResponse {
  workers: Worker[];
  cursor?: {
    cursor: number;
    layer: number;
  };
}

// API functions
export const getWorkers = async (
  componentId: string,
  filter?: WorkerFilter[],
  cursor?: string,
  count?: number,
) => {
  if (!componentId) return { workers: [] };
  const { data } = await apiClient.get<WorkerListResponse>(
    `/v1/components/${componentId}/workers`,
    {
      params: { filter, cursor, count },
    },
  );
  return data;
};

export const getWorker = async (componentId: string, workerName: string) => {
  console.log("called getWorker");
  const { data } = await apiClient.get<Worker>(
    `/v1/components/${componentId}/workers/${workerName}`,
  );
  return data;
};

export interface CreateWorkerPayload {
  name: string;
  args?: string[];
  env?: Record<string, string>;
}

export const createWorker = async (
  componentId: string,
  payload: CreateWorkerPayload,
) => {
  const { data } = await apiClient.post(
    `/v1/components/${componentId}/workers`,
    payload,
  );
  return data;
};

export const invokeWorker = async (
  componentId: string,
  workerName: string,
  functionName: string,
  params: Record<string, unknown>,
) => {
  const { data } = await apiClient.post(
    `/v1/components/${componentId}/workers/${workerName}/invoke-and-await?function=${functionName}`,
    params,
  );
  return data;
};
// export const useInvokeWorker= (
//   options?: UseMutationOptions<
//     void,
//     GolemError,
//     {
//       componentId: string;
//       workerName: string;
//       functionName: string;
//       params: Record<string, unknown>;
//     }
//   >
// ) => {
//   return useMutation({
//     mutationFn: invokeWorker,
//     ...options,
//   });
// };
interface InvokeWorkerVariables {
  componentId: string;
  workerName: string;
  functionName: string;
  params: Record<string, unknown>;
}
export const useInvokeWorker = (
  options?: UseMutationOptions<
    void,
    GolemError,
    InvokeWorkerVariables,
    unknown
  >,
): UseMutationResult<void, GolemError, InvokeWorkerVariables, unknown> => {
  console.log(options);
  return useMutation<void, GolemError, InvokeWorkerVariables, unknown>({
    mutationFn: invokeWorker,
    ...options,
  });
};

export const deleteWorker = async (componentId: string, workerName: string) => {
  const { data } = await apiClient.delete(
    `/v1/components/${componentId}/workers/${workerName}`,
  );
  return data;
};

export const interruptWorker = async (workerId: {
  componentId: string;
  workerName: string;
  recoverImmediately?: boolean;
}) => {
  const { componentId, workerName, recoverImmediately } = workerId;
  const { data } = await apiClient.post(
    `/v1/components/${componentId}/workers/${workerName}/interrupt`,
    null,
    {
      params: { "recovery-immediately": recoverImmediately },
    },
  );
  return data;
};

export const resumeWorker = async (workerId: {
  componentId: string;
  workerName: string;
  recoverImmediately?: boolean;
}) => {
  const { data } = await apiClient.post(
    `/v1/components/${workerId.componentId}/workers/${workerId.workerName}/resume`,
  );
  return data;
};

export const getWorkerLogs = async (
  componentId: string,
  workerName: string,
  count: number,
  cursor?: string,
  query?: string,
) => {
  const { data } = await apiClient.get(
    `/v1/components/${componentId}/workers/${workerName}/oplog`,
    {
      params: { cursor, count, query },
    },
  );
  return data;
};

// Hooks
export const useWorkers = (
  componentId: string,
  filter?: WorkerFilter[],
  cursor?: string,
  count?: number,
): {
  data: WorkerListResponse | undefined;
  isLoading: boolean;
  error: GolemError | null;
} => {
  return useQuery({
    queryKey: workerKeys.list(componentId, { filter, cursor, count }),
    queryFn: () => getWorkers(componentId, filter, cursor, count),
    onError: (error: Error | GolemError) =>
      displayError(error, "Error fetching Workers"),
  });
};

export const useWorker = (
  componentId: string,
  workerName: string,
): {
  data: Worker | undefined;
  isLoading: boolean;
  error: GolemError | null;
} => {
  return useQuery({
    queryKey: workerKeys.detail(componentId, workerName),
    queryFn: () => getWorker(componentId, workerName),
    onError: (error: Error | GolemError) =>
      displayError(error, "Error fetching Worker"),
  });
};

export const useCreateWorker = (componentId: string) => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (payload: CreateWorkerPayload) =>
      createWorker(componentId, payload),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: workerKeys.lists() });
    },
    onError: (error: Error | GolemError) =>
      displayError(error, "Error creating Worker"),
    retry: 0,
  });
};

export const useDeleteWorker = () => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: ({
      componentId,
      workerName,
    }: {
      componentId: string;
      workerName: string;
    }) => deleteWorker(componentId, workerName),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: workerKeys.lists() });
    },
    onError: (error: Error | GolemError) =>
      displayError(error, "Error deleting Worker"),
  });
};

interface InterruptWorkerParams {
  componentId: string;
  workerName: string;
  recoverImmediately?: boolean;
}

export const useInterruptWorker = (
  options?: UseMutationOptions<void, GolemError, InterruptWorkerParams>,
) => {
  const queryClient = useQueryClient();

  return useMutation<void, GolemError, InterruptWorkerParams, unknown>({
    mutationFn: interruptWorker,
    onSuccess: (
      _: void,
      { componentId, workerName }: InterruptWorkerParams,
    ) => {
      // Invalidate specific worker query
      queryClient.invalidateQueries({
        queryKey: workerKeys.detail(componentId, workerName),
      });

      // Invalidate worker list for the component
      queryClient.invalidateQueries({
        queryKey: workerKeys.lists(),
      });
    },
    ...options,
  });
};

export const useResumeWorker = (
  options?: UseMutationOptions<
    void,
    GolemError,
    { componentId: string; workerName: string }
  >,
) => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: resumeWorker,
    onSuccess: (
      _,
      {
        componentId,
        workerName,
      }: {
        componentId: string;
        workerName: string;
      },
    ) => {
      // Invalidate specific worker query
      queryClient.invalidateQueries({
        queryKey: workerKeys.detail(componentId, workerName),
      });

      // Invalidate worker list for the component
      queryClient.invalidateQueries({
        queryKey: workerKeys.lists(),
      });
    },
    onError: (error: Error | GolemError) =>
      displayError(error, "Error resuming Worker"),
    ...options,
  });
};

export const useWorkerLogs = (
  componentId: string,
  workerName: string,
  count: number,
  cursor?: string,
  query?: string,
) => {
  return useQuery({
    queryKey: ["workerLogs", componentId, workerName, count, cursor, query],
    queryFn: () => getWorkerLogs(componentId, workerName, count, cursor, query),
    onError: (error: Error | GolemError) =>
      displayError(error, "Error fetching Worker logs"),
  });
};

export interface WorkerFile {
  name: string;
  lastModified: number;
  kind: "directory" | "file";
  permissions: "read-only" | "read-write";
  size: number;
}

interface WorkerFilesResponse {
  nodes: WorkerFile[];
}

// Query key factory

export const getWorkerFiles = async (
  componentId: string,
  workerName: string,
): Promise<WorkerFilesResponse> => {
  const { data } = await apiClient.get<WorkerFilesResponse>(
    `/v1/components/${componentId}/workers/${workerName}/files`,
  );
  return data;
};

export const downloadWorkerFile = async (
  componentId: string,
  workerName: string,
  fileName: string,
): Promise<Blob> => {
  const { data } = await apiClient.get(
    `/v1/components/${componentId}/workers/${workerName}/file-contents/${fileName}`,
    { responseType: "blob" },
  );
  return data;
};

export const useWorkerFiles = (componentId: string, workerName: string) => {
  return useQuery({
    queryKey: workerKeys.files(componentId, workerName),
    queryFn: () => getWorkerFiles(componentId, workerName),
    onError: (error: Error | GolemError) =>
      displayError(error, "Error fetching Worker files"),
    retry: 0,
  });
};

export interface UpdateWorkerVersionPayload {
  mode: "Automatic";
  targetVersion: number;
}

// Add this API function
export const updateWorkerVersion = async ({
  componentId,
  workerName,
  payload,
}: {
  componentId: string;
  workerName: string;
  payload: UpdateWorkerVersionPayload;
}) => {
  const { data } = await apiClient.post(
    `/v1/components/${componentId}/workers/${workerName}/update`,
    payload
  );
  return data;
};

export const useUpdateWorkerVersion = () => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: updateWorkerVersion,
    onSuccess: (_, { componentId, workerName }:any) => {
      // Invalidate specific worker query
      queryClient.invalidateQueries({
        queryKey: workerKeys.detail(componentId, workerName),
      });

      // Invalidate worker list for the component
      queryClient.invalidateQueries({
        queryKey: workerKeys.lists(),
      });
    },
    onError: (error: Error | GolemError) =>
      displayError(error, "Failed to update worker version"),
  });
};
