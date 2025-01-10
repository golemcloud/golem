/* eslint-disable @typescript-eslint/no-explicit-any */
import DangerZone from "@/components/settings";
import {
  useDeleteWorker,
  interruptWorker,
  resumeWorker,
} from "@/lib/hooks/use-worker";
import { Divider, Paper, Stack, Typography } from "@mui/material";
import {Button2 as Button} from "@/components/ui/button";
import { useParams } from "next/navigation";
import React, { useMemo } from "react";
import { useCustomParam } from "@/lib/hooks/use-custom-param";

export default function Manage() {
  const { compId, id: workerName } = useCustomParam();

  const { deleteWorker } = useDeleteWorker(compId, workerName);

  const actions = useMemo(
    () => [
      {
        title: `Delete this Worker`,
        description:
          "Once you delete a worker, there is no going back. Please be certain.",
        buttonText: `Delete Worker`,
        onClick: (e: any) => {
          e?.preventDefault();
          deleteWorker();
        },
      },
    ],
    [compId, workerName]
  );

  return (
    <div className="mx-auto max-w-7xl px-2 md:px-6 lg:px-8">
      <div className="mx-auto max-w-2xl lg:max-w-none py-4">
        <Paper
          elevation={3}
          sx={{
            p: 3,
            mb: 3,
            borderRadius: 2,
          }}
          className="border"
        >
          <Typography variant="subtitle1">Worker Execution</Typography>
          <Divider className="my-2 bg-border" />
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
                variant="primary"
                size="sm"
                className="text-xs"
                onClick={(e) => {
                  e.preventDefault();
                  interruptWorker(compId, workerName);
                }}
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
                variant="primary"
                size="sm"
                className="text-xs"
                onClick={(e) => {
                  e.preventDefault();
                  resumeWorker(compId, workerName);
                }}
              >
                Resume Worker
              </Button>
            </Stack>
          </Stack>
        </Paper>

        <Paper className="border p-6">
          <DangerZone
            title="Danger Zone"
            description="Proceed with caution."
            actions={actions}
          />
        </Paper>
      </div>
    </div>
  );
}
