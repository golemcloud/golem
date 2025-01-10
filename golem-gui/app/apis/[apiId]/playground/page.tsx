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
  const { apiDefinitions, isLoading,getApiDefintion, error: requestError } = useApiDefinitions(apiId);
  if (isLoading) {
    return <Loader />;
  }
  const {error} = (!isLoading && getApiDefintion() || {});

  return (
    <Paper>
      {(error || requestError) && <ErrorBoundary message={requestError || error} />}
      {!isLoading && !error && (
        <ReactFlowProvider>
          <ReactFlowBuilder apiDefnitions={apiDefinitions} />
        </ReactFlowProvider>
      )}
    </Paper>
  );
}

export default Builder;
