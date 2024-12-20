"use client";
import { Loader } from "lucide-react";
import { useParams } from "next/navigation";
import ReactFlowBuilder from "./ReactFlowBuilder";
import { ReactFlowProvider } from "@xyflow/react";
import useSWR from "swr";
import { fetcher } from "@/lib/utils";
import { ApiDefinition } from "@/types/api";
import { Paper } from "@mui/material";

function Builder() {
  const { apiId } = useParams<{ apiId: string }>();
  const { data, isLoading } = useSWR(
    `?path=api/definitions?api-definition-id=${apiId}`,
    fetcher
  );
  const apiDefintions = (data?.data || []) as ApiDefinition[];
  if (isLoading) {
    return <Loader />;
  }

  return (
    <Paper>
      <ReactFlowProvider>
        <ReactFlowBuilder apiDefnitions={apiDefintions} />
      </ReactFlowProvider>
    </Paper>
  );
}

export default Builder;
