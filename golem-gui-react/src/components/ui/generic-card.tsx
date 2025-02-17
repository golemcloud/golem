import { Box, Typography, Paper } from "@mui/material";
import React from "react";

interface GenericCardProps {
  title: string;
  content?: React.ReactNode;
  emptyMessage?: string;
}

const GenericCard: React.FC<GenericCardProps> = ({ title, content, emptyMessage = "No data found" }) => {
  return (
    <Paper
      elevation={2}
      className="border rounded-sm p-5"
    >
      <Typography
        variant="h6"
        sx={{
          marginBottom: 2,
          fontWeight: "bold",
          fontSize: "0.875rem",
        }}
      >
        {title}
      </Typography>
      <Box
        sx={{
          minHeight: "300px",
          display: "flex",
          justifyContent: "center",
          alignItems: "center",
          borderRadius: 2,
          padding: 2,
        }}
      >
        {content ? (
          content
        ) : (
          <Typography variant="body2" sx={{ color: "#AAAAAA" }}>
            {emptyMessage}
          </Typography>
        )}
      </Box>
    </Paper>
  );
};

export default GenericCard;
