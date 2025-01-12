"use client"

import React from "react";
import {
  Box,
  Typography,
  Grid2 as Grid,
  Paper,
  Divider,
  Stack,
} from "@mui/material";
import { Button2 as Button } from "@/components/ui/button";
import { Pencil,Trash } from "lucide-react";

const ApiDetails = ({route}:{route: any}) => {

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
            try/v0
          </Typography>
        </Grid>

        <Grid size={12}><Divider className="bg-border my-2" /></Grid>

        {/* Path Parameters */}
        <Grid size={{ xs: 12, sm: 3 }} >
          <Typography variant="body2" className="text-muted-foreground">Path Parameters</Typography>
        </Grid>
        
        <Grid size={{ xs: 12, sm: 9 }}>
            <Stack direction="row" gap={5} alignItems="center">
                <Typography className="text-muted-foreground">user_id </Typography>
                <Paper
                    elevation={0}
                    className="w-full"
                    sx={{ p: 2, fontFamily: "monospace", fontSize: "0.875rem" }}
                >
                    u64
                </Paper>
          </Stack>
        </Grid>

        <Grid size={12}><Divider className="bg-border my-2" /></Grid>

        {/* Request Body */}
        <Grid size={{ xs: 12, sm: 3 }} >
          <Typography variant="body2" className="text-muted-foreground">Request Body</Typography>
        </Grid>
        <Grid size={{ xs: 12, sm: 9 }}>
          <Paper
            elevation={0}
            sx={{ p: 2, fontFamily: "monospace", fontSize: "0.875rem" }}
          >
            Value will come from the request body
          </Paper>
        </Grid>

        <Grid size={12}><Divider className="bg-border my-2" /></Grid>

        {/* Response */}
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
            rib will come here
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
            let user: u64 = request.path.user-id;
            <br />
            &#34;my-worker-$&#123;user&#125;&#34;
          </Paper>
        </Grid>
      </Grid>
    </Box>
  );
};

export default ApiDetails;
