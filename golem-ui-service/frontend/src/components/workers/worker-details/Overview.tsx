import {
  Code2,
  HardDrive,
  Link2,
  LucideProps,
  MemoryStick,
  RefreshCcw,
  SquareFunction,
  Timer,
} from "lucide-react";
import { Component, Worker } from "../../../types/api";

import { Link } from "react-router-dom";
import React from "react";

const StatCard = ({
  title,
  value,
  icon: Icon,
}: {
  title: string;
  value: string | number;
  icon: React.ForwardRefExoticComponent<
    Omit<LucideProps, "ref"> & React.RefAttributes<SVGSVGElement>
  >;
}) => (
  <div className="bg-card border border-border/10 rounded-lg p-3 md:p-4 hover:border-border/20 transition-all">
    <div className="flex items-center gap-2 text-muted-foreground mb-1 md:mb-2">
      <Icon size={16} />
      <span className="text-xs md:text-sm">{title}</span>
    </div>
    <div className="text-base md:text-xl font-semibold break-all">{value}</div>
  </div>
);

interface OverviewTabProps {
  worker: Worker;
  component: Component;
}

const Overview: React.FC<OverviewTabProps> = ({ worker, component }) => {
  // Group functions by their exports
  const groupedFunctions = component.metadata?.exports.reduce(
    (acc, exp) => {
      acc[exp.name] = exp.functions;
      return acc;
    },
    {} as Record<
      string,
      Array<{ name: string; parameters: { name: string }[] }>
    >,
  );

  return (
    <div className="space-y-4 md:space-y-6">
      {/* Stats Grid - Responsive layout */}
      <div className="grid grid-cols-2 md:grid-cols-4 gap-2 md:gap-4">
        <StatCard
          title="Memory Usage"
          value={`${(worker.totalLinearMemorySize / 1024 / 1024).toFixed(2)} MB`}
          icon={MemoryStick}
        />
        <StatCard
          title="Component Size"
          value={`${(worker.componentSize / 1024).toFixed(2)} KB`}
          icon={HardDrive}
        />
        <StatCard
          title="Pending Invocations"
          value={worker.pendingInvocationCount}
          icon={Timer}
        />
        <StatCard
          title="Retry Count"
          value={worker.retryCount}
          icon={RefreshCcw}
        />
      </div>

      {/* Exported Functions */}
      <div className="bg-card border border-border/10 rounded-lg p-4 md:p-6">
        <h3 className="text-base md:text-lg font-semibold flex items-center gap-2 mb-4">
          <SquareFunction className="text-primary" size={20} />
          Exported Functions
        </h3>

        <div className="space-y-4 md:space-y-6">
          {Object.entries(groupedFunctions || {}).map(
            ([exportName, functions]) => (
              <div key={exportName} className="space-y-3">
                <div className="flex items-center gap-2 text-muted-foreground">
                  <Code2 size={16} />
                  <h4 className="font-medium text-sm md:text-base">
                    {exportName}
                  </h4>
                </div>

                <div className="grid gap-2">
                  {functions.map((func, index) => (
                    <Link
                      key={index}
                      to={`invoke?functionName=${exportName}.{${func.name}}`}
                      className="flex flex-col sm:flex-row sm:items-center sm:justify-between p-3 bg-card/60 rounded-lg hover:bg-card/80 border border-border/10 hover:border-border/20 transition-all group gap-2"
                    >
                      <div className="space-y-1">
                        <div className="font-medium flex flex-wrap items-center gap-2">
                          <span className="text-sm md:text-base">
                            {func.name}
                          </span>
                          <span className="text-xs md:text-sm text-muted-foreground">
                            ({func.parameters.length} params)
                          </span>
                        </div>

                        {/* Parameter Preview */}
                        {func.parameters.length > 0 && (
                          <div className="text-xs md:text-sm text-muted-foreground break-all">
                            Parameters:{" "}
                            {func.parameters.map((p) => p.name).join(", ")}
                          </div>
                        )}
                      </div>

                      <div className="flex items-center gap-2 text-primary sm:opacity-0 group-hover:opacity-100 transition-opacity">
                        <span className="text-xs md:text-sm">Invoke</span>
                        <Link2 size={16} />
                      </div>
                    </Link>
                  ))}
                </div>
              </div>
            ),
          )}

          {(!groupedFunctions ||
            Object.keys(groupedFunctions).length === 0) && (
            <div className="text-center py-6 md:py-8 text-muted-foreground">
              <Code2 size={24} className="mx-auto mb-2 opacity-50" />
              <p className="text-sm md:text-base">
                No exported functions available
              </p>
              <p className="text-xs md:text-sm mt-1">
                This worker has no exposed functions to invoke
              </p>
            </div>
          )}
        </div>
      </div>
    </div>
  );
};

export default Overview;
