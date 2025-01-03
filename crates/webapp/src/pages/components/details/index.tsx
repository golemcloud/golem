import { useParams } from "react-router-dom";
import { MetricCard } from "./widgets/metrixCard";
import { ExportsList } from "./widgets/exportsList";
import { WorkerStatus } from "./widgets/workerStatus";
import ComponentLeftNav from "./componentsLeftNav";
import { useEffect, useState } from "react";
import { API } from "@/service";
import { Component } from "@/types/component.ts";
import { Worker, WorkerStatus as IWorkerStatus } from "@/types/worker.ts";
import ErrorBoundary from "@/components/errorBoundary";

export const ComponentDetails = () => {
  const { componentId } = useParams();
  const [component, setComponent] = useState({} as Component);
  const [workerStatus, setWorkerStatus] = useState({} as IWorkerStatus);

  useEffect(() => {
    API.getComponentById(componentId!).then((res) => {
      setComponent(res);
    });

    API.findWorker(componentId!).then((res) => {
      const status: Record<string, number> = {};
      res.workers.forEach((worker: Worker) => {
        status[worker.status] = (status[worker.status] || 0) + 1;
      });
      setWorkerStatus(status);
    });
  }, [componentId]);

  return (
    <ErrorBoundary>
      <div className="flex">
        <ComponentLeftNav />
        <div className="flex-1 flex flex-col">
          <header className="w-full border-b bg-background py-4">
            <div className="mx-auto px-6 lg:px-8">
              <div className="flex items-center gap-4">
                <h1 className="text-xl font-semibold text-foreground truncate">
                  {componentId}
                </h1>
              </div>
            </div>
          </header>
          <div className="flex-1 p-8">
            <div className="p-6 max-w-7xl mx-auto space-y-6">
              <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-4">
                <MetricCard
                  title="Latest Component Version"
                  value={
                    "v" + (component?.versionedComponentId?.version || "0")
                  }
                  type="version"
                />
                <MetricCard
                  title="Active Workers"
                  value={
                    (workerStatus.Running || 0) +
                    (workerStatus.Idle || 0) +
                    (workerStatus.Failed || 0)
                  }
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

              <div className="grid gap-4 md:grid-cols-2">
                <ExportsList exports={component?.metadata?.exports[0]} />
                <WorkerStatus workerStatus={workerStatus} />
              </div>
            </div>
          </div>
        </div>
      </div>
    </ErrorBoundary>
  );
};
