"use client";
import RoutePage from "@/components/api-route-page";
import DeploymentPage from "@/components/deployment";
import { Typography, Paper } from "@mui/material";
import { useParams, useSearchParams } from "next/navigation";

export default function Overview() {
  const { apiId } = useParams<{ apiId: string }>();
  const params = useSearchParams();
  const version = params.get("version");


  //TODO: we can make this api overview simialr to components tab structure so that we will have more control over the data
  return (
    <>
      {/* Routes Section */}
      <>
        <Paper
          elevation={3}
         
          sx={{
            p: 3,
            mb: 3,
            borderRadius: 2,
          }}
          className="border"
        >
          <Typography variant="h6" gutterBottom>
            Routes
          </Typography>
            <RoutePage apiId={apiId} limit={5} version={version} />
        </Paper>
      </>
      {/* Active Deployments Section */}
      <DeploymentPage apiId={apiId} limit={5} />
    </>
  );
}
