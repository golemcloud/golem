import ErrorBoundary from "@/components/errorBoundary";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { cn, formatRelativeTime } from "@/lib/utils";
import { API } from "@/service";
import { ComponentList } from "@/types/component";
import { Worker } from "@/types/worker";
import { LayoutGrid, PlusCircle } from "lucide-react";
import React, { useCallback, useEffect, useMemo, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";

/**
 * Worker status metrics used to categorize workers
 */
const WORKER_STATUS_METRICS = [
  "Idle",
  "Running",
  "Suspended",
  "Failed",
] as const;

const WORKER_COLOR_MAPPER = {
  Idle: "text-emerald-400 dark:text-emerald-200",
  Running: "text-blue-400 dark:text-blue-200",
  Suspended: "text-amber-400 dark:text-amber-200",
  Failed: "text-rose-400 dark:text-rose-200",
};

type WorkerStatusType = (typeof WORKER_STATUS_METRICS)[number];

/**
 * Debounce delay (in milliseconds) for search functionality
 */
const SEARCH_DEBOUNCE_MS = 300;

/**
 * Shape of the worker status for any single component
 */
type ComponentWorkerStatus = {
  [K in WorkerStatusType]: number;
};

/**
 * Mapping of component IDs to their worker statuses
 */
type WorkerStatusMap = {
  [key: string]: ComponentWorkerStatus;
};

/**
 * Mapping of component IDs to their details
 */
type ComponentMap = {
  [key: string]: ComponentList;
};

/**
 * Default worker status used when no workers or statuses are found
 * for a given component.
 */
const DEFAULT_WORKER_STATUS: ComponentWorkerStatus =
  WORKER_STATUS_METRICS.reduce((acc, metric) => {
    acc[metric] = 0;
    return acc;
  }, {} as ComponentWorkerStatus);

/**
 * Card representing a single component's details and worker status
 */
export const ComponentCard = React.memo(
  ({
    data,
    workerStatus,
    onCardClick,
  }: {
    data: ComponentList;
    workerStatus?: ComponentWorkerStatus;
    onCardClick: (componentId: string) => void;
  }) => {
    // Retrieve the latest version from the versions array
    const latestVersion = data.versions?.[data.versions?.length - 1];
    // Count total exports using a helper function
    const exportCount = (latestVersion?.metadata?.exports as string[]) || [];
    // Convert component size from bytes to kilobytes
    const componentSize = Math.round(
      (latestVersion?.componentSize || 0) / 1024,
    );

    /**
     * Handles a click on the entire card.
     * Only triggers if componentId is present.
     */
    const handleClick = () => {
      if (data.componentId) {
        onCardClick(data.componentId);
      }
    };

    return (
      <Card
        className={cn(
          "w-full max-w-2xl from-background to-muted border-border cursor-pointer bg-gradient-to-br hover:shadow-lg",
        )}
        onClick={handleClick}
      >
        <CardHeader>
          <CardTitle className="text-foreground font-mono">
            {data.componentName || "Unnamed Component"}
          </CardTitle>
          <CardDescription
            className={"text-xs font-light text-muted-foreground"}
          >
            {formatRelativeTime(
              data.versions?.[data.versions?.length - 1]!.createdAt ||
                new Date(),
            )}
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-6">
          {/* Worker Status Grid */}
          {/*
            Removed the extra ":grid-cols-4" class which appeared to be a typo.
            Adjust classes to a responsive 2-column (mobile) to 4-column (desktop) layout.
          */}
          {workerStatus && (
            <div className="grid grid-cols-4 gap-4">
              {WORKER_STATUS_METRICS.map(metric => (
                <div key={metric} className="space-y-2">
                  <h3
                    className={cn(
                      "text-sm font-mono",
                      WORKER_COLOR_MAPPER[metric],
                    )}
                  >
                    {metric}
                  </h3>
                  <p
                    className={cn(
                      "text-xl font-bold",
                      WORKER_COLOR_MAPPER[metric],
                    )}
                  >
                    {workerStatus[metric]}
                  </p>
                </div>
              ))}
            </div>
          )}

          {/* Component Metadata Badges */}
          <div className="flex flex-wrap gap-2">
            <Badge
              variant="secondary"
              className="bg-muted hover:bg-muted/80 text-muted-foreground transition-colors rounded-md cursor-pointer shadow"
            >
              V{data.versionList?.[data.versionList?.length - 1] || "0"}
            </Badge>
            <Badge
              variant="secondary"
              className="bg-muted hover:bg-muted/80 text-muted-foreground transition-colors rounded-md cursor-pointer shadow"
            >
              {exportCount || 0} Exports
            </Badge>
            <Badge
              variant="secondary"
              className="bg-muted hover:bg-muted/80 text-muted-foreground transition-colors rounded-md cursor-pointer shadow"
            >
              {Math.round((componentSize || 0) / 1024)} KB{" "}
            </Badge>
            <Badge
              variant="secondary"
              className="bg-muted hover:bg-muted/80 text-muted-foreground transition-colors rounded-md cursor-pointer shadow"
            >
              {data.componentType}
            </Badge>
          </div>
        </CardContent>
      </Card>
    );
  },
);

ComponentCard.displayName = "ComponentCard";

/**
 * Main component for listing and searching project components
 */
const Components = () => {
  const navigate = useNavigate();
  const [componentList, setComponentList] = useState<ComponentMap>({});
  const [filteredComponents, setFilteredComponents] = useState<ComponentMap>(
    {},
  );
  const [workerList, setWorkerList] = useState<WorkerStatusMap>({});
  const [searchQuery, setSearchQuery] = useState("");
  const { appId } = useParams<{ appId: string }>();

  /**
   * Fetch all components, then fetch worker status for each component in parallel
   */
  const fetchComponentsAndMetrics = useCallback(async () => {
    try {
      const response = await API.componentService.getComponentByIdAsKey(appId!);
      setComponentList(response);
      setFilteredComponents(response);

      const componentStatus: WorkerStatusMap = {};

      // Map over each component to fetch worker info
      const workerPromises = Object.values(response).map(async comp => {
        if (comp.componentId) {
          const worker = await API.workerService.findWorker(
            appId!,
            comp.componentId,
            {
              count: 100,
              precise: true,
            },
          );

          // Initialize status with all metrics set to 0
          const status = { ...DEFAULT_WORKER_STATUS };

          // Update counts for existing statuses
          worker.workers.forEach((w: Worker) => {
            const wStatus = w.status as WorkerStatusType;
            if (wStatus && status[wStatus] !== undefined) {
              status[wStatus] += 1;
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

  /**
   * On mount, fetch components and their worker statuses
   */
  useEffect(() => {
    (async () => {
      await fetchComponentsAndMetrics();
    })();
  }, []);

  /**
   * Debounce-based search filter for components by name
   */
  useEffect(() => {
    const timeoutId = setTimeout(() => {
      if (!searchQuery) {
        setFilteredComponents(componentList);
        return;
      }

      // Filter matches where the component name includes the user’s search text
      const filtered = Object.entries(componentList).reduce(
        (acc, [key, component]) => {
          const componentName = component.componentName?.toLowerCase() || "";
          if (componentName.includes(searchQuery.toLowerCase())) {
            acc[key] = component;
          }
          return acc;
        },
        {} as ComponentMap,
      );

      setFilteredComponents(filtered);
    }, SEARCH_DEBOUNCE_MS);

    return () => clearTimeout(timeoutId);
  }, [searchQuery, componentList]);

  /**
   * Memoized empty state component to render when no components are found
   */
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
    [],
  );

  /**
   * Handler for card click, navigates to the individual component details page
   */
  const handleCardClick = useCallback(
    (componentId: string) => {
      navigate(`/app/${appId}/components/${componentId}`);
    },
    [navigate],
  );

  return (
    <ErrorBoundary>
      <div className="container mx-auto px-4 py-8">
        {/* Page Header */}
        <div className="flex justify-between items-center mb-6">
          <h1 className="text-2xl font-bold">Components</h1>
          <div className="flex gap-4">
            <div className="w-64">
              {/* Search Input */}
              <Input
                type="text"
                placeholder="Search components..."
                value={searchQuery}
                onChange={e => setSearchQuery(e.target.value)}
                className="w-full"
              />
            </div>
            {/* Create Component Button */}
            <Button onClick={() => navigate(`/app/${appId}/components/create`)}>
              <PlusCircle className="h-4 w-4 mr-2" />
              Create Component
            </Button>
          </div>
        </div>

        {/* Main Content: Grid of components or empty state */}
        {Object.keys(filteredComponents).length === 0 ? (
          EmptyState
        ) : (
          <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-6 overflow-scroll max-h-[78vh] px-4">
            {Object.values(filteredComponents).map(data => (
              <ComponentCard
                key={data.componentId}
                data={data}
                workerStatus={
                  workerList[data.componentId || ""] || DEFAULT_WORKER_STATUS
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
