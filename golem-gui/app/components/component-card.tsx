import React from "react";
import { Box, Typography, Stack } from "@mui/material";

interface ComponentCardProps {
  name: string;
  time: string;
  version: number;
  exports: number;
  size: string;
  type: string;
  onClick: () => void;
}

const ComponentCard: React.FC<ComponentCardProps> = ({
  name,
  time,
  version,
  exports,
  size,
  type,
  onClick,
}) => {
  return (
    <Box
      sx={{
        p: 2,
        maxHeight: "fit-content",
        display: "flex",
        flexDirection: "column",
        cursor: "pointer",
        gap: 1,
        minWidth: "300px",
        "&:hover": { boxShadow: "0px 5px 10px 0px #555" },
      }}
      onClick={onClick}
      className="flex-1 border rounded-md"
    >
      <Box
        sx={{
          display: "flex",
          justifyContent: "space-between",
          alignItems: "center",
          mb: 1,
          gap:2
        }}
      >
        <Stack sx={{
          maxWidth:"80%"
        }}>
            <Typography variant="subtitle1"
            sx={{
              overflow: "hidden",
              textOverflow: "ellipsis",
              whiteSpace: "nowrap",
              fontWeight: 500,
            }}
            >
                {name}
            </Typography>
            <Typography variant="caption" color="#888">
                {time}
            </Typography>
        </Stack>
        
        <Typography className=" bg-button_bg border border-button_border px-2  rounded-sm text-sm">
          {"v"+version}
        </Typography>
      </Box>

      <Stack direction="row" spacing={1} sx={{ mt: 1 }}>
        <Typography variant="body1" className="border px-2 rounded-md text-muted-foreground">
            { exports+" Exports"}
        </Typography>
        <Typography variant="body1" className="border px-2 rounded-md text-muted-foreground">
            {size + " MB"}
        </Typography>
        <Typography variant="body1" className="border px-2 rounded-md text-muted-foreground">
            {type}
        </Typography>
      </Stack>
    </Box>
  );
};

export default ComponentCard;
