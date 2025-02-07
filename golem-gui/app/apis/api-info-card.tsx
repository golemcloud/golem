import React from "react";
import { Box, Typography, Stack, Card } from "@mui/material";
import LockIcon from "@mui/icons-material/Lock";
import LockOpenIcon from "@mui/icons-material/LockOpen";
import { Button2 } from "@/components/ui/button";
import { GitCommitHorizontal } from "lucide-react";

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
        borderRadius: 2,
        maxHeight: "fit-content",
        display: "flex",
        flexDirection: "column",
        cursor: "pointer",
        gap: 1,
        minWidth: "300px",
        "&:hover": { cursor: "pointer", boxShadow: "0px 3px 10px 1px #666"
        },
      }}
      onClick={onClick}
      className="flex-1 border"
    >
      {" "}
      <Box
        sx={{
          display: "flex",
          justifyContent: "space-between",
          alignItems: "center",
        }}
      >
        <Typography variant="subtitle1" fontWeight="bold"
        sx={{
          overflow: "hidden",
          textOverflow: "ellipsis",
          whiteSpace: "nowrap",
          maxWidth: "80%",
        }}
        >
          {name}
        </Typography>
        <Button2
          variant="default"
          endIcon={<GitCommitHorizontal />}
          size="xs"
          className="px-2"
        >
          {routesCount}
        </Button2>
      </Box>
      <Stack
        direction="row"
        justifyContent="space-between"
        alignItems="center"
        sx={{ mt: 1 }}
      >
        <Stack direction="column">
          <Typography variant="body2" sx={{ fontSize: "12px" }}
          className="text-muted-foreground"
          >

            Latest Version
          </Typography>
          <Typography
            variant="body2"
            sx={{
              border: "1px solid #555",
              width: "fit-content",
              marginTop:"1px",
              padding: "1px 5px",
              borderRadius: "4px",
            }}
            className="text-muted-foreground"
          >
            {version}
          </Typography>
        </Stack>
        <Stack direction="column">
          <Typography variant="body2" sx={{ fontSize: "12px" }}
          className="text-muted-foreground"
          >
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
              className="text-muted-foreground"
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
