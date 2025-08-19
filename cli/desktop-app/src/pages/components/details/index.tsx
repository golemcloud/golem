import { useParams } from "react-router-dom";
import { useEffect, useState, useMemo } from "react";
import { API } from "@/service";
import { MetricCard } from "./widgets/metrixCard";
import { ExportsList } from "./widgets/exportsList";
import { WorkerStatus } from "./widgets/workerStatus";

import { ComponentList } from "@/types/component";
import { Worker, WorkerStatus as IWorkerStatus } from "@/types/worker";

export const ComponentDetails = () => {
  const { componentId = "", appId } = useParams();
  const [component, setComponent] = useState<ComponentList | null>(null);
  const [workerStatus, setWorkerStatus] = useState<IWorkerStatus>({});
  const [error, setError] = useState<Error | null>(null);

  useEffect(() => {
    if (!componentId) return;

    // Fetch component info and worker status in parallel
    Promise.all([
      API.componentService.getComponentByIdAsKey(appId!),
      API.workerService.findWorker(appId!, componentId),
    ])
      .then(([componentMap, workerResponse]) => {
        // 1. Set the component data
        const foundComponent = componentMap[componentId] || null;
        setComponent(foundComponent);

        // 2. Build a worker status map
        const status: IWorkerStatus = {
          Idle: 0,
          Running: 0,
          Suspended: 0,
          Failed: 0,
        };
        workerResponse.workers.forEach((worker: Worker) => {
          status[worker.status as keyof IWorkerStatus] =
            (status[worker.status as keyof IWorkerStatus] || 0) + 1;
        });
        setWorkerStatus(status);
      })
      .catch(err => {
        console.error("Error fetching component/worker data:", err);
        setError(err);
      });
  }, [componentId]);

  /**
   * Safely compute metrics even if 'component' is null.
   * For example, if the API data is still loading or the ID is invalid.
   */
  const latestVersion = useMemo(() => {
    const versionList = component?.versionList || [];
    return versionList[versionList.length - 1] || 0;
  }, [component]);

  const activeWorkers = useMemo(() => {
    return (
      (workerStatus.Running || 0) +
      (workerStatus.Idle || 0) +
      (workerStatus.Failed || 0)
    );
  }, [workerStatus]);

  // Optional: you could handle error states or loading states
  if (error) {
    return (
      <div className="p-8 text-red-500">
        Failed to load component data. Please try again later.
      </div>
    );
  }

  if (!component) {
    return null;
  }

  return (
    <div className="flex">
      <div className="flex-1 p-8">
        {component.componentType === "Durable" ? (
          <div className="p-6 max-w-7xl mx-auto space-y-6">
            {/* Metrics Row */}
            <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-4">
              <MetricCard
                title="Latest Component Version"
                value={`v${latestVersion}`}
                type="version"
              />
              <MetricCard
                title="Active Workers"
                value={activeWorkers}
                type="active"
              />
              <MetricCard
                title="Running Workers"
                value={workerStatus.Running || 0}
                type="running"
              />
              <MetricCard
                title="Failed Workers"
                value={workerStatus.Failed || 0}
                type="failed"
              />
            </div>

            {/* Exports & Worker Status */}
            <div
              className={`grid gap-4 ${
                component.componentType === "Durable" ? "md:grid-cols-2" : ""
              }`}
            >
              <ExportsList
                exports={
                  component.versions?.[component.versions.length - 1]
                    ?.exports || []
                }
              />
              {component.componentType === "Durable" && (
                <WorkerStatus workerStatus={workerStatus} />
              )}
            </div>
          </div>
        ) : (
          <div className="p-6 max-w-3xl mx-auto  space-y-6">
            <MetricCard
              title="Latest Component Version"
              value={`v${latestVersion}`}
              type="version"
            />
            <div className={`grid gap-4`}>
              <ExportsList
                exports={
                  component.versions?.[component.versions.length - 1]?.metadata
                    ?.exports || []
                }
              />
            </div>
          </div>
        )}
      </div>
    </div>
  );
};
