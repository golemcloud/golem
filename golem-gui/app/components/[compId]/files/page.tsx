"use client";
import React from "react";
import { Box, Typography, Grid2 as Grid, Paper } from "@mui/material";
import { Folder } from "lucide-react";
// import { useWorkerFileContent } from "@/lib/hooks/use-worker";
import SecondaryHeader from "@/components/ui/secondary-header";
import ErrorBoundary from "@/components/error/error-boundary";
import { useCustomParam } from "@/lib/hooks/use-custom-param";
import FileTree from "./file-explorer";

const FileComponent = () => {
  const { compId } = useCustomParam();
  // const { data, isLoading, error } = useWorkerFileContent(
  //   "test",
  //   compId,
  //   "file-service.wasm"
  // ) as { data: unknown; isLoading: boolean; error?: string | null };

  // console.log(data, isLoading);

  return (
    <>
      <Box sx={{ display: { xs: "block", md: "none" } }}>
        <SecondaryHeader onClick={() => {}} variant="components" />
        {/* {error && <ErrorBoundary message={error} />} */}
      </Box>
        <div className="mx-auto max-w-7xl px-2 md:px-6 lg:px-8">
        <div className="mx-auto max-w-2xl lg:max-w-none py-4">
          <FileTree/>
      </div>
      </div>
    </>
  );
};

export default FileComponent;
