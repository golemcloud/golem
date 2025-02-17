import React from "react";
import { Box, Typography, Stack, Card } from "@mui/material";
import LockIcon from "@mui/icons-material/Lock";
import LockOpenIcon from "@mui/icons-material/LockOpen";
import { Button2 } from "../ui/button";
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
      onClick={onClick}
      className="flex-1 border  rounded-md  p-4 max-h-fit flex flex-col cursor-pointer gap-1 min-w-[300px] hover:hover:shadow-custom  hover:cursor-pointer"
    >
      {" "}
      <Box className="flex justify-between items-center">
        <Typography
          variant="subtitle1"
          fontWeight="bold"
          className="overflow-hidden text-ellipsis whitespace-nowrap max-w-[80%]"
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
        className="mt-1"
      >
        <Stack direction="column">
          <Typography variant="body2" className="text-muted-foreground text-xs">
            Latest Version
          </Typography>
          <Typography
            variant="body2"
            className="text-muted-foreground border w-fit mt-[1px] px-[5px] py-[1px] rounded-md"
          >
            {version}
          </Typography>
        </Stack>
        <Stack direction="column">
          <Typography variant="body2" className="text-muted-foreground text-xs">
            Routes
          </Typography>
          <Stack direction="row">
            <Box className="flex items-center gap-1">
              {locked ? (
                <LockIcon className="text-[1.2rem] text-muted-foreground " />
              ) : (
                <LockOpenIcon className="text-[1.2rem] text-muted-foreground  " />
              )}
            </Box>
            <Typography
              variant="body2"
              className="text-muted-foreground p-[4px_2px]"
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
