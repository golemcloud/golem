"use client";

import React, { useState } from "react";
import { Box, Typography, Grid2 as Grid, Paper, Divider } from "@mui/material";
import { Button2 as Button } from "@/components/ui/button";
import { Pencil, Trash } from "lucide-react";
import { ApiRoute } from "@/types/api";
import TryItOut from "./try-it-out";
import CustomModal from "./CustomModal";
import NewRouteForm from "./new-route";
import useApiDefinitions from "@/lib/hooks/use-api-definitons";
import { useCustomParam } from "@/lib/hooks/use-custom-param";
import { AlertDialogDemo } from "./confirmation-dialog";
import { useRouter } from "next/navigation";

const ApiDetails = ({
  route,
  version,
}: {
  route: ApiRoute;
  version: string;
}) => {
  const { apiId } = useCustomParam();
  const router = useRouter();
  const { deleteRoute } = useApiDefinitions(apiId);
  const handleDelete = async (
    e: React.MouseEvent<HTMLButtonElement>
  ): Promise<void> => {
    console.log("delete route");
    e.preventDefault();
    try {
      await deleteRoute(route!, version);
      router.push(`/apis/${apiId}/overview?version=${version}`);
    } catch (error) {
      throw error;
    }
  };
  const [open,setOpen]=useState(false)
  return (
    <>
    <Box>
      <Box className="flex justify-between">
        <Box>
          <Typography variant="h5">{route?.path}</Typography>
          <Button variant="primary" size="icon_sm">
            {route?.method}
          </Button>
        </Box>
        <Box >
          <Button variant="primary" size="sm" endIcon={<Pencil size={64}/>} onClick={() => setOpen((prev) => !prev)}>
            Edit
          </Button>
          <AlertDialogDemo
            onSubmit={(e: any) => handleDelete(e)}
            paragraph={
              "This action cannot be undone. This will permanently delete this route."
            }
            child={
              <Button
                variant="error"
                size="sm"
                endIcon={<Trash />}
                className="ml-2"
              >
                {" "}
                Delete{" "}
              </Button>
            }
          />
        </Box>
      </Box>

      {/* Sections */}
      <Grid container spacing={2}>
        {/* Component */}
        <Grid size={12}>
          <Divider className="bg-border my-2" />
        </Grid>
        <Grid size={{ xs: 12, sm: 3 }} alignItems="center">
          <Typography variant="body2" className="text-muted-foreground">
            Component
          </Typography>
        </Grid>
        <Grid size={{ xs: 12, sm: 9 }} alignItems="center">
          <Typography variant="body2" fontFamily="monospace">
            {route?.binding?.componentId?.componentId}
            {"/"}
            {route?.binding?.componentId?.version}
          </Typography>
        </Grid>

        <Grid size={12}>
          <Divider className="bg-border my-2" />
        </Grid>
        <Grid size={12}>
          <TryItOut route={route} version={version} />
        </Grid>
        <Grid size={{ xs: 12, sm: 3 }}>
          <Typography variant="body2">
            <Box display="flex" flexDirection="column" gap={1}>
              <span className="text-muted-foreground">Response</span>
              <Button
                variant="primary"
                size="icon_sm"
                className="font-mono w-fit"
              >
                Rib
              </Button>
            </Box>
          </Typography>
        </Grid>

        <Grid size={{ xs: 12, sm: 9 }}>
          <Paper
            elevation={0}
            sx={{ p: 2, fontFamily: "monospace", fontSize: "0.875rem" }}
          >
            {route?.binding?.response}
          </Paper>
        </Grid>

        <Grid size={12}>
          <Divider className="bg-border my-2" />
        </Grid>

        {/* Worker Name */}
        <Grid size={{ xs: 12, sm: 3 }}>
          <Typography variant="body2">
            <Box display="flex" flexDirection="column" gap={1}>
              <span className="text-muted-foreground">Worker Name</span>
              <Button
                variant="primary"
                size="icon_sm"
                className="font-mono w-fit"
              >
                Rib
              </Button>
            </Box>
          </Typography>
        </Grid>
        <Grid size={{ xs: 12, sm: 9 }}>
          <Paper
            elevation={0}
            sx={{ p: 2, fontFamily: "monospace", fontSize: "0.875rem" }}
          >
            {route?.binding?.workerName}
            <br />
          </Paper>
        </Grid>
      </Grid>
    </Box>
       <CustomModal open={open} onClose={() => {}}>
       {route && (
         <NewRouteForm
           apiId={route.binding.componentId.componentId}
           version={route.binding.componentId.version}
           defaultRoute={route}
           onSuccess={() => {}}
         />
       )}
     </CustomModal>
     </>
  );
};

export default ApiDetails;
