import React from "react";
import { Box, Typography, Grid, Paper } from "@mui/material";
import FolderIcon from "@mui/icons-material/Folder";

const NoFilesComponent = () => {
  return (
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
        <FolderIcon sx={{ fontSize: 60, color: "#757575", marginBottom: 1 }} />
        <Typography variant="body2">No files found</Typography>
      </Box>
    </Paper>
  );
};

export default NoFilesComponent;
