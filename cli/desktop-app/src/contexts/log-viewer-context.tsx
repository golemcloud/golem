import { createContext, useContext, useState, ReactNode } from "react";

interface LogViewerData {
  title: string;
  logs: string;
  status: "success" | "error" | "info";
  operation: string;
}

interface LogViewerContextType {
  isOpen: boolean;
  logData: LogViewerData;
  showLog: (data: LogViewerData) => void;
  showErrorLog: (title: string, logs: string, operation: string) => void;
  hideLog: () => void;
}

const LogViewerContext = createContext<LogViewerContextType | undefined>(
  undefined,
);

export function LogViewerProvider({ children }: { children: ReactNode }) {
  const [isOpen, setIsOpen] = useState(false);
  const [logData, setLogData] = useState<LogViewerData>({
    title: "",
    logs: "",
    status: "info",
    operation: "",
  });

  const showLog = (data: LogViewerData) => {
    setLogData(data);
    setIsOpen(true);
  };

  const showErrorLog = (title: string, logs: string, operation: string) => {
    showLog({
      title,
      logs,
      status: "error",
      operation,
    });
  };

  const hideLog = () => {
    setIsOpen(false);
  };

  return (
    <LogViewerContext.Provider
      value={{
        isOpen,
        logData,
        showLog,
        showErrorLog,
        hideLog,
      }}
    >
      {children}
    </LogViewerContext.Provider>
  );
}

export function useLogViewer() {
  const context = useContext(LogViewerContext);
  if (context === undefined) {
    throw new Error("useLogViewer must be used within a LogViewerProvider");
  }
  return context;
}
