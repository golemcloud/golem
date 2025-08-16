import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { CircleSlash, Clock } from "lucide-react";
import { WorkerStatus as IWorkerStatus } from "@/types/worker.ts";
import * as React from "react";
import { Label, Pie, PieChart } from "recharts";
import {
  ChartConfig,
  ChartContainer,
  ChartTooltip,
  ChartTooltipContent,
} from "@/components/ui/chart";
import { Separator } from "@/components/ui/separator.tsx";
import ErrorBoundary from "@/components/errorBoundary";

// Chart configuration with type safety via `satisfies`
const chartConfig = {
  value: {
    label: "Worker Count",
  },
  Idle: {
    label: "Idle",
    color: "hsl(var(--chart-1))",
  },
  Running: {
    label: "Running",
    color: "hsl(var(--chart-2))",
  },
  Suspended: {
    label: "Suspended",
    color: "hsl(var(--chart-3))",
  },
  Failed: {
    label: "Failed",
    color: "hsl(var(--chart-4))",
  },
} satisfies ChartConfig;

export function WorkerStatus({
  workerStatus,
}: {
  workerStatus: IWorkerStatus;
}) {
  // Calculate total workers and memoize the result
  const total = React.useMemo(() => {
    return Object.values(workerStatus).reduce((acc, val) => acc + val, 0);
  }, [workerStatus]);

  // Prepare data for the pie chart from workerStatus
  const pieData = React.useMemo(
    () =>
      Object.entries(workerStatus).map(([key, value]) => ({
        key,
        value,
      })),
    [workerStatus],
  );

  // Extracted render function for the chart label for clarity
  const renderChartLabel = ({
    viewBox,
  }: {
    viewBox?: { cx?: number; cy?: number };
  }) => {
    if (viewBox && "cx" in viewBox && "cy" in viewBox) {
      return (
        <text
          x={viewBox.cx}
          y={viewBox.cy}
          textAnchor="middle"
          dominantBaseline="middle"
        >
          <tspan
            x={viewBox.cx}
            y={viewBox.cy}
            className="fill-foreground text-3xl font-bold"
          >
            {total.toLocaleString()}
          </tspan>
          <tspan
            x={viewBox.cx}
            y={(viewBox.cy || 0) + 24}
            className="fill-muted-foreground"
          >
            Total Workers
          </tspan>
        </text>
      );
    }
    // Optionally return null or fallback content if viewBox is undefined
    return null;
  };

  return (
    <ErrorBoundary>
      <Card className="flex flex-col">
        <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
          <CardTitle className="text-base font-medium">Worker Status</CardTitle>
          <Clock className="h-4 w-4 text-muted-foreground" />
        </CardHeader>
        <Separator className="m-4 mx-auto" />
        {pieData.length === 0 || pieData.every(item => item.value === 0) ? (
          <CardContent className="flex-1 pb-0 flex items-center justify-center">
            <div className="flex flex-col items-center text-muted-foreground">
              <CircleSlash className="w-10 h-10" />
              <p className="mt-2 text-sm">No worker data available</p>
            </div>
          </CardContent>
        ) : (
          <CardContent className="flex-1 pb-0">
            <ChartContainer
              config={chartConfig}
              className="mx-auto aspect-square max-h-[250px]"
            >
              <PieChart>
                <ChartTooltip
                  cursor={false}
                  content={<ChartTooltipContent hideLabel />}
                />
                <Pie
                  data={pieData}
                  dataKey="value"
                  nameKey="key"
                  innerRadius={60}
                  strokeWidth={5}
                >
                  {/* @ts-ignore */}
                  <Label content={renderChartLabel} />
                </Pie>
              </PieChart>
            </ChartContainer>
          </CardContent>
        )}
      </Card>
    </ErrorBoundary>
  );
}
