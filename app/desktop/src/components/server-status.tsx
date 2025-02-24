import { cn } from "@/lib/utils";
import { API } from "@/service";
import { AlertCircle, CheckCircle2 } from "lucide-react";
import { useEffect, useState } from "react";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";

interface HealthStatus {
  status: "healthy" | "unhealthy";
  timestamp: string;
  uptime: number;
  error?: string;
}

export function ServerStatus() {
  const [status, setStatus] = useState<HealthStatus | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    const checkHealth = async () => {
      try {
        await API.checkHealth();
        setStatus({
          status: "healthy",
          timestamp: new Date().toISOString(),
          uptime: 0,
        });
      } catch (error) {
        setStatus({
          status: "unhealthy",
          timestamp: new Date().toISOString(),
          uptime: 0,
          error: "Failed to fetch server status",
        });
      } finally {
        setLoading(false);
      }
    };

    checkHealth();
    // Check status every 30 seconds
    const interval = setInterval(checkHealth, 30000);
    return () => clearInterval(interval);
  }, []);

  if (loading) {
    return (
      <div className="flex items-center gap-2 px-3 py-1.5 text-sm">
        <div className="h-2 w-2 animate-pulse rounded-full bg-muted-foreground" />
        Checking status...
      </div>
    );
  }

  const statusContent =
    status && status?.status[0].toUpperCase() + status?.status.slice(1);

  return (
    <TooltipProvider>
      <Tooltip>
        <TooltipTrigger>
          <div
            className={cn(
              "flex items-center gap-2 px-3 py-1.5 text-sm",
              status?.status === "healthy" ? "text-green-500" : "text-red-500",
            )}
          >
            {status?.status === "healthy" ? (
              <CheckCircle2 className="h-4 w-4" />
            ) : (
              <AlertCircle className="h-4 w-4" />
            )}
            <span>{statusContent}</span>
          </div>
        </TooltipTrigger>
        <TooltipContent>Server is {statusContent}</TooltipContent>
      </Tooltip>
    </TooltipProvider>
  );
}
