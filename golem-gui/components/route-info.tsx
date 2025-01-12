"use client"

import React, { useMemo } from "react";
import {
  Box,
  Typography,
  Grid2 as Grid,
  Paper,
  Divider,
  Stack,
} from "@mui/material";
import { Button2 as Button } from "@/components/ui/button";
import { Loader, Pencil,Trash } from "lucide-react";
import { ApiRoute, Component } from "@/types/api";
import useComponents from "@/lib/hooks/use-component";
import TryItOut from "./try-it-out";

const ApiDetails = ({route}:{route: ApiRoute}) => {

  // console.log("route================>", route?.binding?.componentId?.componentId);
  // const {error:requestError, getComponent, isLoading} = useComponents(route?.binding?.componentId?.componentId, route?.binding?.componentId?.version);

  // const {component, error} = useMemo(()=>{
  //   return (!isLoading && !requestError && getComponent())
  // },[getComponent, isLoading, requestError])

  // console.log("component======>", component);

  // if(isLoading){
  //   return <Loader/>
  // }

  return (
    <Box>
      <Box className="flex justify-between">
        <Box>
          <Typography variant="h5" >
           {route?.path}
          </Typography>
          <Button variant="primary" size="icon_sm">
            {route?.method}
          </Button>
        </Box>
        <Box >
          <Button variant="primary" size="sm" endIcon={<Pencil size={64}/>}>
            Edit
          </Button>
          <Button variant="error" size="sm" endIcon={<Trash/>} className="ml-2">
            Delete
          </Button>
        </Box>
      </Box>


      {/* Sections */}
      <Grid container spacing={2}>
        {/* Component */}
        <Grid size={12}><Divider className="bg-border my-2" /></Grid>
        <Grid size={{ xs: 12, sm: 3 }} alignItems="center">
          <Typography variant="body2" className="text-muted-foreground">Component</Typography>
        </Grid>
        <Grid size={{ xs: 12, sm: 9 }} alignItems="center">
          <Typography variant="body2" fontFamily="monospace">
            {route?.binding?.componentId?.componentId}{"/"}{route?.binding?.componentId?.version}
          </Typography>
        </Grid>

        <Grid size={12}><Divider className="bg-border my-2" /></Grid>
        <Grid size={12}>
        <TryItOut route={route}/>
        </Grid>
        <Grid size={{ xs: 12, sm: 3 }} >
          <Typography variant="body2">
            <Box display="flex" flexDirection="column" gap={1}>
              <span className="text-muted-foreground">Response</span>
              <Button variant="primary" size="icon_sm" className="font-mono w-fit">Rib</Button>
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

        <Grid size={12}><Divider className="bg-border my-2" /></Grid>

        {/* Worker Name */}
        <Grid size={{ xs: 12, sm: 3 }} >
        <Typography variant="body2">
            <Box display="flex" flexDirection="column" gap={1}>
              <span className="text-muted-foreground">Worker Name</span>
              <Button variant="primary" size="icon_sm" className="font-mono w-fit">Rib</Button>
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
  );
};

export default ApiDetails;
