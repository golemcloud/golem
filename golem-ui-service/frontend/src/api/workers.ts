import {
  UseMutationOptions,
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { Worker, WorkerStatus } from "../types/api";

import { GolemError } from "../types/error";
import { apiClient } from "../lib/api-client";

// Query keys
export const workerKeys = {
  all: ["workers"] as const,
  lists: () => [...workerKeys.all, "list"] as const,
  list: (componentId: string, filters: Record<string, unknown>) =>
    [...workerKeys.lists(), componentId, filters] as const,
  details: () => [...workerKeys.all, "detail"] as const,
  detail: (componentId: string, workerName: string) =>
    [...workerKeys.details(), componentId, workerName] as const,
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
  count?: number
) => {
  const { data } = await apiClient.get<WorkerListResponse>(
    `/v1/components/${componentId}/workers`,
    {
      params: { filter, cursor, count },
    }
  );
  return data;
};

export const getWorker = async (componentId: string, workerName: string) => {
  const { data } = await apiClient.get<Worker>(
    `/v1/components/${componentId}/workers/${workerName}`
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
  payload: CreateWorkerPayload
) => {
  const { data } = await apiClient.post(
    `/v1/components/${componentId}/workers`,
    payload
  );
  return data;
};

export const deleteWorker = async (componentId: string, workerName: string) => {
  const { data } = await apiClient.delete(
    `/v1/components/${componentId}/workers/${workerName}`
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
    }
  );
  return data;
};

export const resumeWorker = async (workerId: {
  componentId: string;
  workerName: string;
  recoverImmediately?: boolean;
}) => {
  const { data } = await apiClient.post(
    `/v1/components/${workerId.componentId}/workers/${workerId.workerName}/resume`
  );
  return data;
};

// Hooks
export const useWorkers = (
  componentId: string,
  filter?: WorkerFilter[],
  cursor?: string,
  count?: number
) => {
  return useQuery({
    queryKey: workerKeys.list(componentId, { filter, cursor, count }),
    queryFn: () => getWorkers(componentId, filter, cursor, count),
  });
};

export const useWorker = (componentId: string, workerName: string) => {
  return useQuery({
    queryKey: workerKeys.detail(componentId, workerName),
    queryFn: () => getWorker(componentId, workerName),
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
    onSuccess: (_, { componentId }) => {
      queryClient.invalidateQueries({ queryKey: workerKeys.lists() });
    },
  });
};

interface InterruptWorkerParams {
  componentId: string;
  workerName: string;
  recoverImmediately?: boolean;
}

export const useInterruptWorker = (
  options?: UseMutationOptions<void, GolemError, InterruptWorkerParams>
) => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: interruptWorker,
    onSuccess: (
      _,
      {
        componentId,
        workerName,
      }: {
        componentId: string;
        workerName: string;
      }
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
  >
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
      }
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
