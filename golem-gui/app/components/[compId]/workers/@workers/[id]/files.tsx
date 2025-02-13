"use client";
import React from "react";
import { Box, Typography, Grid2 as Grid, Paper } from "@mui/material";
import { Folder } from "lucide-react";

const FileComponent = () => {
  // const { compId } = useCustomParam();
  // const { data, isLoading, error } = useWorkerFileContent(
  //   "test",
  //   compId,
  //   "file-service.wasm"
  // ) as { data: unknown; isLoading: boolean; error?: string | null };

  return (
    <>
      {/* {error && <ErrorBoundary message={error} />} */}

      <div className="mx-auto max-w-7xl px-2 md:px-6 lg:px-8">
        <div className="mx-auto max-w-2xl lg:max-w-none py-4">
          <Paper
            elevation={3}
            sx={{
              backgroundColor: "#1c1c1c",
              borderRadius: "8px",
              overflow: "hidden",
              height: "80%",
            }}
          >
            <Grid
              container
              sx={{
                padding: "10px 16px",
              }}
              className="dark:bg-[#2c2c2c] bg-[#ebebeb]"
            >
              <Grid size={{ xs: 6 }}>
                <Typography variant="body1">NAME</Typography>
              </Grid>
              <Grid size={{ xs: 6 }} textAlign="right">
                <Typography variant="body1">PERMISSIONS</Typography>
              </Grid>
            </Grid>

            <Box
              sx={{
                display: "flex",
                alignItems: "center",
                justifyContent: "center",
                flexDirection: "column",
                padding: "30px",
              }}
            >
              <Folder size={48} />
              <Typography variant="body2" className="text-foreground">
                No files found
              </Typography>
            </Box>
          </Paper>
        </div>
      </div>
    </>
  );
};

export default FileComponent;
