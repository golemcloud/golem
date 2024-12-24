import React from "react";
import { Box, Typography, Chip, Stack, Card } from "@mui/material";
import LockIcon from "@mui/icons-material/Lock";
import LockOpenIcon from "@mui/icons-material/LockOpen";

interface ApiInfoProps {
  name: string;
  version: string;
  routesCount: number;
  locked: boolean;
  onClick: () => void;
}

const ApiInfoCard: React.FC<ApiInfoProps> = ({
  name,
  version,
  routesCount,
  locked,
  onClick,
}) => {
  return (
    <Card
      sx={{
        p: 2,
        border: "1px solid #666",
        borderRadius: 2,
        width: "320px",
        display: "flex",
        flexDirection: "column",
        cursor: "pointer",
        gap: 1,
        "&:hover": { boxShadow: 2 },
      }}
      onClick={onClick}
    >   <Box
        sx={{
          display: "flex",
          justifyContent: "space-between",
          alignItems: "center",
        }}
      >
        <Typography variant="subtitle1" fontWeight="bold">
          {name}
        </Typography>
        <Chip
          label=  {routesCount}
          sx={{
            fontSize: "0.8rem",
            fontWeight: 500,
            borderRadius: 1,
            bgcolor: "primary.main",
            color: "primary.contrastText",
          }}
        />
      </Box>

      <Stack
        direction="row"
        justifyContent="space-between"
        alignItems="center"
        sx={{ mt: 1 }}
      >
        <Stack direction="column" >
            <Typography variant="body2" sx={{fontSize:"12px"}}>
                Latest Version
            </Typography>
            <Typography
            variant="body2"
            sx={{
                border: "1px solid #555",
                width: "fit-content",
                padding: "8px 2px",
                borderRadius: "4px",
            }}
            >
            {version}
            </Typography>
        </Stack>
        <Stack direction="column" >
            <Typography variant="body2" sx={{fontSize:"12px"}}>
                Routes
            </Typography>
            <Stack direction="row">
            <Box sx={{ display: "flex", alignItems: "center", gap: 1 }}>
          {locked ? (
            <LockIcon sx={{ fontSize: "1.2rem", color: "#888" }} />
          ) : (
            <LockOpenIcon sx={{ fontSize: "1.2rem", color: "#888" }} />
          )}
        </Box>
            <Typography
            variant="body2"
            sx={{
                padding: "4px 2px",
            }}
            >
            {routesCount}
            </Typography>
         
        </Stack>
        </Stack>
       
      </Stack>
    </Card>
  );
};

export default ApiInfoCard;
