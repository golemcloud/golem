import { Card } from "@/components/ui/card";
import { Worker } from "@/types/api";
import { Box, Typography, Grid2 as Grid, Stack } from "@mui/material";
import React from "react";

export default function ErrorPage({ worker }: { worker: Worker }) {
  return (
    <div className="mx-auto max-w-7xl px-2 md:px-6 lg:px-8">
      <div className="mx-auto max-w-2xl lg:max-w-none py-4">
        <Box className={"space-y-4"}>
          <div className="bg-white dark:bg-[#281619] p-6 rounded-lg shadow-lg border dark:border-[#6a1d25] border-[#f04444]">
            <h2 className="text-lg font-bold text-foreground mb-2">
              Worker Failure
            </h2>
            <p>
              A critical error occurred in the worker. Please review the details
              below.
            </p>
          </div>
          <Card className="px-4 py-6">
            <Typography variant="body1" mb={2}>
              Worker Details
            </Typography>
            <Grid container spacing={2}  sx={{
        gridTemplateColumns: {
          xs: "1fr", // Single column for small screens
          md: "repeat(2, 1fr)", // Two columns for medium screens and larger
        },
        display: "grid",
      }}>
              <Grid>
                <Stack>
                  <Typography>Component ID</Typography>
                  <Typography>{worker?.workerId?.componentId}</Typography>
                </Stack>
              </Grid>
              <Grid>
                <Stack>
                  <Typography>Worker Name</Typography>
                  <Typography>{worker?.workerId?.workerName}</Typography>
                </Stack>
              </Grid>

              <Grid>
                {" "}
                <Stack>
                  <Typography>Component Version</Typography>
                  <Typography>{worker.componentVersion}</Typography>
                </Stack>{" "}
              </Grid>
              <Grid>
                {" "}
                <Stack>
                  <Typography>Created At</Typography>
                  <Typography>{worker.createdAt}</Typography>
                </Stack>{" "}
              </Grid>
              <Grid>
                {" "}
                <Stack>
                  <Typography>Retry Count</Typography>
                  <Typography>{worker.retryCount}</Typography>
                </Stack>{" "}
              </Grid>
              <Grid>
                {" "}
                <Stack>
                  <Typography>Status</Typography>
                  <Typography color="red">Failed</Typography>
                </Stack>{" "}
              </Grid>
            </Grid>
          </Card>
          <Card className="px-4 py-6 container">
            <Typography variant="body1" mb={2}>
              Last Error
            </Typography>
            <Typography variant="body2" className="text-wrap">{worker.lastError}</Typography>
          </Card>
        </Box>
      </div>
    </div>
  );
}
