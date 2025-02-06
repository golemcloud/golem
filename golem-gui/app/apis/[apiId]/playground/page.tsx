"use client";
import { Loader } from "lucide-react";
import ReactFlowBuilder from "./ReactFlowBuilder";
import { ReactFlowProvider } from "@xyflow/react";
import { Box } from "@mui/material";
import useApiDefinitions from "@/lib/hooks/use-api-definitons";
import ErrorBoundary from "@/components/error-boundary";
import { useCustomParam } from "@/lib/hooks/use-custom-param";

function Builder() {
  const { apiId } = useCustomParam();
  // const params = useSearchParams();
  // const version = params.get("version");
  const { apiDefinitions, isLoading,getApiDefintion, error: requestError } = useApiDefinitions(apiId);
  if (isLoading) {
    return <Loader />;
  }
  const {error} = (!isLoading && getApiDefintion() || {});

  return (
    <Box className="absolute w-full left-0 top-28">
      {(error || requestError) && <ErrorBoundary message={requestError || error} />}
      {!isLoading && !error && (
        <ReactFlowProvider>
          <ReactFlowBuilder apiDefnitions={apiDefinitions} />
        </ReactFlowProvider>
      )}
    </Box>
  );
}

export default Builder;
