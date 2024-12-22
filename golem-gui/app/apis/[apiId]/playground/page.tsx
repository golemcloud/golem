"use client";
import { Loader } from "lucide-react";
import { useParams, useSearchParams } from "next/navigation";
import ReactFlowBuilder from "./ReactFlowBuilder";
import { ReactFlowProvider } from "@xyflow/react";
import useSWR from "swr";
import { fetcher } from "@/lib/utils";
import { ApiDefinition } from "@/types/api";
import { Paper } from "@mui/material";
import useApiDefinitions from "@/lib/hooks/use-api-definitons";

function Builder() {
  const { apiId } = useParams<{ apiId: string }>();
  const params = useSearchParams();
  const version = params.get("version");
  const { apiDefinitions, isLoading } = useApiDefinitions(apiId);
  if (isLoading) {
    return <Loader />;
  }

  return (
    <Paper>
      <ReactFlowProvider>
        <ReactFlowBuilder apiDefnitions={apiDefinitions} />
      </ReactFlowProvider>
    </Paper>
  );
}

export default Builder;
