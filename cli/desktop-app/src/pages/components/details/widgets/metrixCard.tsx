import ErrorBoundary from "@/components/errorBoundary";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Layers, Activity, Play, AlertCircle } from "lucide-react";

interface MetricCardProps {
  title: string;
  value: string | number;
  type: "version" | "active" | "running" | "failed";
}

export function MetricCard({ title, value, type }: MetricCardProps) {
  const icons = {
    version: <Layers className="h-4 w-4 text-muted-foreground" />,
    active: <Activity className="h-4 w-4 text-blue-500" />,
    running: <Play className="h-4 w-4 text-green-500" />,
    failed: <AlertCircle className="h-4 w-4 text-red-500" />,
  };

  return (
    <ErrorBoundary>
      <Card>
        <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
          <CardTitle className="text-sm font-medium">{title}</CardTitle>
          {icons[type]}
        </CardHeader>
        <CardContent>
          <div className="text-2xl font-bold">{value}</div>
        </CardContent>
      </Card>
    </ErrorBoundary>
  );
}
