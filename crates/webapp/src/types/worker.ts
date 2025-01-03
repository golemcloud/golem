export interface Worker {
    accountId: string;
    activePlugins: any[];
    args: any[];
    componentSize: number;
    componentVersion: number;
    createdAt: string;
    env: { [key: string]: string };
    lastError: any;
    ownedResources: any;
    pendingInvocationCount: number;
    retryCount: number;
    status: string;
    totalLinearMemorySize: number;
    updates: any[];
    workerId: {
        componentId: string;
        workerName: string;
    };
}

export interface WorkerStatus {
    Idle?: number;
    Running?: number;
    Suspended?: number;
    Failed?: number;
}
  