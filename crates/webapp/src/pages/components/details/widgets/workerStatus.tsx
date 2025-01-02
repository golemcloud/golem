import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Clock } from "lucide-react";
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
  const total = React.useMemo(() => {
    return Object.values(workerStatus).reduce((acc, val) => acc + val, 0);
  }, [workerStatus]);

  return (
    <ErrorBoundary>
      <Card className="flex flex-col">
        <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
          <CardTitle className="text-base font-medium">Worker Status</CardTitle>
          <Clock className="h-4 w-4 text-muted-foreground" />
        </CardHeader>
        <Separator className="m-4 mx-auto" />
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
                data={Object.entries(workerStatus).map(([key, value]) => ({
                  key,
                  value,
                }))}
                dataKey="value"
                nameKey="key"
                innerRadius={60}
                strokeWidth={5}
              >
                <Label
                  content={({ viewBox }) => {
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
                  }}
                />
              </Pie>
            </PieChart>
          </ChartContainer>
        </CardContent>
      </Card>
    </ErrorBoundary>
  );
}
