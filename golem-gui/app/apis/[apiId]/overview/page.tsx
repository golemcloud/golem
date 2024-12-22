"use client";
import RoutePage from "@/components/api-route-page";
import DeploymentPage from "@/components/deployment";
import { Box, Typography, Paper } from "@mui/material";
import { useParams, useSearchParams } from "next/navigation";
import useApiDefinitions from "@/lib/hooks/use-api-definitons";
import { Loader } from "lucide-react";

export default function Overview() {
  const { apiId } = useParams<{ apiId: string}>();
  const params = useSearchParams();
  const version = params.get("version");
  const { getApiDefintion, isLoading } = useApiDefinitions(apiId);
  const { data: apiDefinition, error } = getApiDefintion(apiId, version);

  if (isLoading) {
    return <Loader />;
  }
  return (
    <>
      {/* Routes Section */}
      {apiDefinition && !error && (
        <>
          <Paper
            elevation={3}
            className="bg-[#333]"
            sx={{
              p: 3,
              mb: 3,
              color: "text.primary",
              border: 1,
              borderColor: "divider",
              borderRadius: 2,
            }}
          >
            <Typography variant="h6" gutterBottom>
              Routes
            </Typography>
            {/*TODO: Dynamically update the route Page on version change. currently it is showing latest */}
            <RoutePage apiId={apiId} limit={5}/>

            {error && (
              <Typography className="text-rose-500">{error}</Typography>
            )}
          </Paper>
          {/* Active Deployments Section */}
        </>
      )}
    <DeploymentPage apiId={apiId} limit={5} />
    </>
  );
}
