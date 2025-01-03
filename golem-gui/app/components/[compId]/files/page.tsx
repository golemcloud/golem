"use client";
import React from "react";
import { Box, Typography, Grid, Paper } from "@mui/material";
import FolderIcon from "@mui/icons-material/Folder";
import { useWorkerFileContent } from "@/lib/hooks/use-worker";
import { useParams } from "next/navigation";
import SecondaryHeader from "@/components/ui/secondary-header";
import ErrorBoundary from "@/components/erro-boundary";

const NoFilesComponent = () => {
  const { compId } = useParams<{ compId: string }>();
  const { data, isLoading, error } = useWorkerFileContent(
    "test",
    compId,
    "file-service.wasm"
  );

  console.log(data, isLoading);

  return (
    <> <Box sx={{ display: { xs: "block", md: "none" } }}>
    <SecondaryHeader onClick={() => {}} variant="components" />
  </Box>
  {error && <ErrorBoundary message={error}/>}  
    <div className="mx-auto max-w-7xl px-6 lg:px-8">
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
            backgroundColor: "#2c2c2c",
            color: "#ffffff",
          }}
        >
          <Grid item xs={6}>
            <Typography variant="body1" fontWeight="bold">
              NAME
            </Typography>
          </Grid>
          <Grid item xs={6} textAlign="right">
            <Typography variant="body1" fontWeight="bold">
              PERMISSIONS
            </Typography>
          </Grid>
        </Grid>

        <Box
          sx={{
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            flexDirection: "column",
            height: "calc(100% - 50px)",
            color: "#ffffff",
          }}
        >
          <FolderIcon
            sx={{ fontSize: 60, color: "#757575", marginBottom: 1 }}
          />
          <Typography variant="body2">No files found</Typography>
        </Box>
      </Paper>
    </div>
    </div>
    </>
  );
};

export default NoFilesComponent;
