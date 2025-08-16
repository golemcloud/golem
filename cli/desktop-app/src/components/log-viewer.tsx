import { useState } from "react";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Copy, CheckCircle, AlertCircle, Info } from "lucide-react";
import { toast } from "@/hooks/use-toast";

interface LogViewerProps {
  isOpen: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  logs: string;
  status: "success" | "error" | "info";
  operation: string;
}

export function LogViewer({
  isOpen,
  onOpenChange,
  title,
  logs,
  status,
  operation,
}: LogViewerProps) {
  const [isCopied, setIsCopied] = useState(false);

  const handleCopy = async () => {
    try {
      await navigator.clipboard.writeText(logs);
      setIsCopied(true);
      toast({
        title: "Copied to clipboard",
        description: "Logs have been copied to your clipboard.",
      });
      setTimeout(() => setIsCopied(false), 2000);
    } catch {
      toast({
        title: "Failed to copy",
        description: "Could not copy logs to clipboard.",
        variant: "destructive",
      });
    }
  };

  const getStatusIcon = () => {
    switch (status) {
      case "success":
        return <CheckCircle className="h-4 w-4 text-green-500" />;
      case "error":
        return <AlertCircle className="h-4 w-4 text-red-500" />;
      case "info":
        return <Info className="h-4 w-4 text-blue-500" />;
    }
  };

  const getStatusColor = () => {
    switch (status) {
      case "success":
        return "bg-green-100 text-green-800 dark:bg-green-900 dark:text-green-300";
      case "error":
        return "bg-red-100 text-red-800 dark:bg-red-900 dark:text-red-300";
      case "info":
        return "bg-blue-100 text-blue-800 dark:bg-blue-900 dark:text-blue-300";
    }
  };

  return (
    <Dialog open={isOpen} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-6xl w-[95vw] h-[80vh] flex flex-col">
        <DialogHeader className="flex flex-row items-center justify-between space-y-0 pb-4 mr-4">
          <div className="flex items-center gap-3">
            <div className="flex items-center gap-2">
              {getStatusIcon()}
              <DialogTitle className="text-lg">{title}</DialogTitle>
            </div>
            <Badge variant="outline" className={getStatusColor()}>
              {operation}
            </Badge>
          </div>
          <Button
            variant="outline"
            size="sm"
            onClick={handleCopy}
            className="text-xs"
          >
            {isCopied ? (
              <CheckCircle className="h-3 w-3 mr-1" />
            ) : (
              <Copy className="h-3 w-3 mr-1" />
            )}
            {isCopied ? "Copied!" : "Copy"}
          </Button>
        </DialogHeader>

        <div className="flex-1 border rounded-md bg-muted/10 overflow-hidden">
          <ScrollArea className="h-full w-full max-h-[60vh]">
            <div className="p-4">
              <pre className="text-sm font-mono whitespace-pre-wrap break-words leading-relaxed">
                {logs || "No output available"}
              </pre>
            </div>
          </ScrollArea>
        </div>

        <div className="pt-4 text-xs text-muted-foreground">
          <p>
            {status === "error"
              ? "Operation failed. Review the logs above for details."
              : status === "success"
                ? "Operation completed successfully."
                : "Operation output displayed above."}
          </p>
        </div>
      </DialogContent>
    </Dialog>
  );
}
