import { LogViewer } from "@/components/log-viewer";
import { useLogViewer } from "@/contexts/log-viewer-context";

export function GlobalLogViewer() {
  const { isOpen, logData, hideLog } = useLogViewer();

  return (
    <LogViewer
      isOpen={isOpen}
      onOpenChange={hideLog}
      title={logData.title}
      logs={logData.logs}
      status={logData.status}
      operation={logData.operation}
    />
  );
}
