import React, { useCallback, useEffect, useMemo, useState } from "react";
import { LayoutGrid, PlusCircle } from "lucide-react";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";

import { useNavigate } from "react-router-dom";
import { API } from "@/service";
import { ComponentList } from "@/types/component";
import { Worker } from "@/types/worker";
import ErrorBoundary from "@/components/errorBoundary";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";
import { calculateExportFunctions } from "@/lib/utils";

// Constants
const WORKER_STATUS_METRICS = [
  "Idle",
  "Running",
  "Suspended",
  "Failed",
] as const;
type WorkerStatusType = (typeof WORKER_STATUS_METRICS)[number];
const SEARCH_DEBOUNCE_MS = 300;

// Types
type ComponentWorkerStatus = {
  [K in WorkerStatusType]: number;
};

type WorkerStatusMap = {
  [key: string]: ComponentWorkerStatus;
};

type ComponentMap = {
  [key: string]: ComponentList;
};

/**
 * Component card that displays component information
 */
const ComponentCard = React.memo(
  ({
    data,
    workerStatus,
    onCardClick,
  }: {
    data: ComponentList;
    workerStatus: ComponentWorkerStatus;
    onCardClick: (componentId: string) => void;
  }) => {
    const latestVersion = data.versions?.[data.versions?.length - 1];
    const exportCount = calculateExportFunctions(
      latestVersion?.metadata?.exports || []
    ).length;
    const componentSize = Math.round(
      (latestVersion?.componentSize || 0) / 1024
    );

    // Ensure componentId exists before calling onCardClick
    const handleClick = () => {
      if (data.componentId) {
        onCardClick(data.componentId);
      }
    };

    return (
      <Card className="border shadow-sm cursor-pointer" onClick={handleClick}>
        <CardHeader>
          <CardTitle>{data.componentName || "Unnamed Component"}</CardTitle>
        </CardHeader>
        <CardContent className="space-y-4">
          {/* Worker Status Section */}
          <div className="grid grid-cols-2 sm:grid-cols-4 :grid-cols-4  gap-2">
            {WORKER_STATUS_METRICS.map((metric) => (
              <div key={metric} className="flex flex-col items-start space-y-1">
                <span className="text-sm text-muted-foreground">{metric}</span>
                <span className="text-lg font-medium">
                  {workerStatus[metric]}
                </span>
              </div>
            ))}
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <Badge variant="secondary" className="rounded-md">
              V{data.versionList?.[data.versionList?.length - 1] || "0"}
            </Badge>
            <Badge variant="secondary" className="rounded-md">
              {exportCount} Exports
            </Badge>
            <Badge variant="secondary" className="rounded-md">
              {componentSize} KB
            </Badge>
            <Badge variant="secondary" className="rounded-md">
              {latestVersion?.componentType || "Unknown"}
            </Badge>
          </div>
        </CardContent>
      </Card>
    );
  }
);

ComponentCard.displayName = "ComponentCard";

/**
 * Components page that displays a list of all components
 */
const Components = () => {
  const navigate = useNavigate();
  const [componentList, setComponentList] = useState<ComponentMap>({});
  const [filteredComponents, setFilteredComponents] = useState<ComponentMap>(
    {}
  );
  const [workerList, setWorkerList] = useState<WorkerStatusMap>({});
  const [searchQuery, setSearchQuery] = useState("");

  // Fetch components and their metrics
  const fetchComponentsAndMetrics = useCallback(async () => {
    try {
      const response = await API.getComponentByIdAsKey();
      setComponentList(response);
      setFilteredComponents(response);

      // Fetch worker status for each component
      const componentStatus: WorkerStatusMap = {};
      const workerPromises = Object.values(response).map(async (comp) => {
        if (comp.componentId) {
          const worker = await API.findWorker(comp.componentId, {
            count: 100,
            precise: true,
          });

          // Initialize status with all metrics set to 0
          const status = WORKER_STATUS_METRICS.reduce((acc, metric) => {
            acc[metric] = 0;
            return acc;
          }, {} as ComponentWorkerStatus);

          // Update counts for existing statuses
          worker.workers.forEach((worker: Worker) => {
            const workerStatus = worker.status as WorkerStatusType;
            if (workerStatus && status[workerStatus] !== undefined) {
              status[workerStatus] += 1;
            }
          });

          componentStatus[comp.componentId] = status;
        }
      });

      await Promise.all(workerPromises);
      setWorkerList(componentStatus);
    } catch (error) {
      console.error("Error fetching components or metrics:", error);
    }
  }, []);

  useEffect(() => {
    fetchComponentsAndMetrics();
  }, [fetchComponentsAndMetrics]);

  // Filter components based on search query
  useEffect(() => {
    const timeoutId = setTimeout(() => {
      if (!searchQuery) {
        setFilteredComponents(componentList);
        return;
      }

      const filtered = Object.entries(componentList).reduce(
        (acc, [key, component]) => {
          const componentName = component.componentName?.toLowerCase() || "";
          if (componentName.includes(searchQuery.toLowerCase())) {
            acc[key] = component;
          }
          return acc;
        },
        {} as ComponentMap
      );

      setFilteredComponents(filtered);
    }, SEARCH_DEBOUNCE_MS);

    return () => clearTimeout(timeoutId);
  }, [searchQuery, componentList]);

  // Memoize the empty state component
  const EmptyState = useMemo(
    () => (
      <div className="border-2 border-dashed border-gray-200 rounded-lg p-12 flex flex-col items-center justify-center">
        <div className="h-16 w-16 bg-gray-100 rounded-lg flex items-center justify-center mb-4">
          <LayoutGrid className="h-8 w-8 text-gray-400" />
        </div>
        <h2 className="text-xl font-semibold mb-2 text-center">
          No Project Components
        </h2>
        <p className="text-gray-500 mb-6 text-center">
          Create a new component to get started.
        </p>
      </div>
    ),
    []
  );

  const handleCardClick = useCallback(
    (componentId: string) => {
      navigate(`/components/${componentId}`);
    },
    [navigate]
  );

  return (
    <ErrorBoundary>
      <div className="container mx-auto px-4 py-8">
        <div className="flex justify-between items-center mb-6">
          <h1 className="text-2xl font-bold">Components</h1>
          <div className="flex gap-4">
            <div className="w-64">
              <Input
                type="text"
                placeholder="Search components..."
                value={searchQuery}
                onChange={(e) => setSearchQuery(e.target.value)}
                className="w-full"
              />
            </div>
            <Button onClick={() => navigate("/components/create")}>
              <PlusCircle className="h-4 w-4 mr-2" />
              Create Component
            </Button>
          </div>
        </div>

        {Object.keys(filteredComponents).length === 0 ? (
          EmptyState
        ) : (
          <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-6 overflow-scroll max-h-[78vh]">
            {Object.values(filteredComponents).map((data) => (
              <ComponentCard
                key={data.componentId}
                data={data}
                workerStatus={
                  workerList[data.componentId || ""] ||
                  WORKER_STATUS_METRICS.reduce((acc, metric) => {
                    acc[metric] = 0;
                    return acc;
                  }, {} as ComponentWorkerStatus)
                }
                onCardClick={handleCardClick}
              />
            ))}
          </div>
        )}
      </div>
    </ErrorBoundary>
  );
};

export default Components;
