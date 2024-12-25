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
      sx={{
        backgroundColor: "#1E1E1E",
        color: "#FFFFFF",
        padding: 3,
        borderRadius: 2,
      }}
    >
      {/* Title */}
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

      {/* Content or Empty State */}
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
