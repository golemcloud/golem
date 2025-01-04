"use client";
import { Loader } from "lucide-react";
import { useParams } from "next/navigation";
import ReactFlowBuilder from "./ReactFlowBuilder";
import { ReactFlowProvider } from "@xyflow/react";
import { Paper } from "@mui/material";
import useApiDefinitions from "@/lib/hooks/use-api-definitons";
import ErrorBoundary from "@/components/erro-boundary";

function Builder() {
  const { apiId } = useParams<{ apiId: string }>();
  // const params = useSearchParams();
  // const version = params.get("version");
  const { apiDefinitions, isLoading, error } = useApiDefinitions(apiId);
  if (isLoading) {
    return <Loader />;
  }

  return (
    <Paper>
      {error && <ErrorBoundary message={error} />}
      {!isLoading && !error && (
        <ReactFlowProvider>
          <ReactFlowBuilder apiDefnitions={apiDefinitions} />
        </ReactFlowProvider>
      )}
    </Paper>
  );
}

export default Builder;
