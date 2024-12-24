import React from "react";
import { Box, Typography, Button, Chip, Stack } from "@mui/material";

interface ComponentCardProps {
  name: string;
  date: string;
  version: string;
  exports: number;
  size: string;
  type: string;
}

const ComponentCard: React.FC<ComponentCardProps> = ({
  name,
  date,
  version,
  exports,
  size,
  type,
}) => {
  return (
    <Box
      sx={{
        p: 2,
        mb: 2,
        border: "1px solid #555",
        borderRadius: 2,
        display: "flex",
        flexDirection: "column",
        cursor: "pointer",
        gap: 1,
        minWidth: "300px",
        "&:hover": { boxShadow: 4 },
      }}
      className="flex-1"
    >
      <Box
        sx={{
          display: "flex",
          justifyContent: "space-between",
          alignItems: "center",
          mb: 1,
        }}
      >
        <Box>
            <Typography variant="subtitle1" sx={{ fontWeight: 500 }}>
                {name}
            </Typography>
            <Typography variant="caption" color="#888">
                {date}
            </Typography>
        </Box>
        
        <Chip
          label={version}
          sx={{
            fontSize: "0.8rem",
            fontWeight: 500,
            borderRadius: 1,
            bgcolor: "primary.main",
            color: "primary.contrastText",
          }}
        />
      </Box>

      <Stack direction="row" spacing={1} sx={{ mt: 1 }}>
        <Typography variant="body1" className="border border-[#555] px-2 rounded-md">
            { exports+" Exports"}
        </Typography>
        <Typography variant="body1" className="border border-[#555] px-2 rounded-md">
            {size}
        </Typography>
        <Typography variant="body1" className="border border-[#555] px-2 rounded-md">
            {type}
        </Typography>
      </Stack>
    </Box>
  );
};

export default ComponentCard;
