"use client";
import RoutePage from "../../api-route-page";
import DeploymentPage from "../../deployment";
import { useCustomParam } from "@/lib/hooks/use-custom-param";
import { Typography, Paper } from "@mui/material";
import { useSearchParams } from "next/navigation";

export default function Overview() {
  const { apiId } = useCustomParam();
  const params = useSearchParams();
  const version = params.get("version");


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
