import DangerZone from "@/components/settings";
import { useDeleteWorker, interruptWorker, resumeWorker } from "@/lib/hooks/use-worker";
import {
  Box,
  Button,
  Divider,
  Paper,
  Stack,
  Typography,
} from "@mui/material";
import { useParams } from "next/navigation";
import React, { useMemo } from "react";

export default function Manage() {
  const { compId, id: workerName } = useParams<{
    compId: string;
    id: string;
  }>();

  const {deleteWorker} = useDeleteWorker(compId, workerName);

  const actions = useMemo(()=>[
    {
      title: `Delete this Worker`,
      description: "Once you delete a worker, there is no going back. Please be certain.",
      buttonText: `Delete Worker`,
      onClick: (e:any) => {e?.preventDefault(); deleteWorker()},
    },
  ], [compId, workerName]);

  return (
    <Box
      sx={{height: "100vh", margin:"auto" }}
      className="text-black dark:text-white md:w-[80%] w-full"
    >
      <Paper
      elevation={3}
      className="bg-[#333]"
      sx={{
        p: 3,
        mt:3,
        mb: 3,
        color: "text.primary",
        border: 1,
        borderColor: "divider",
        borderRadius: 2,
      }}
      >
        <Typography variant="subtitle1">Worker Execution</Typography>
        <Divider sx={{my:2}}/>
        <Stack gap={4}>
        <Typography variant="caption">
          Manage the worker and its execution.
        </Typography>

        <Stack
          direction={"row"}
          justifyContent={"space-between"}
          alignItems={"center"}
        >
          <Stack>
            <Typography>Interrupt Worker</Typography>
            <Typography variant="caption">
              Interrupts the execution of a running worker
            </Typography>
          </Stack>
          <Button
            onClick={(e) => {
              e.preventDefault();
              interruptWorker(compId, workerName);
            }}
            variant={"outlined"}
            size="small"
          >
            Interrupt Worker
          </Button>
        </Stack>
        <Stack
          direction={"row"}
          justifyContent={"space-between"}
          alignItems={"center"}
        >
          <Stack>
            <Typography>Resume Worker</Typography>
            <Typography variant="caption">
              Resumes the execution of an interrupted worker
            </Typography>
          </Stack>
          <Button
            onClick={(e) => {
              e.preventDefault();
              resumeWorker(compId, workerName);
            }}
            variant={"outlined"}
            size="small"
          >
            Resume Worker
          </Button>
        </Stack>
        </Stack>
      </Paper>

      <Paper>
      <DangerZone
        title="Danger Zone"
        description="Proceed with caution."
        actions={actions}
      />
      </Paper>
    </Box>
  );
}
