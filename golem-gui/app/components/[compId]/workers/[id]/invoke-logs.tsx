import React, { useCallback, useMemo } from "react";
import { useWorkerLogs } from "@/lib/hooks/use-worker";
import { useParams } from "next/navigation";
import {
  Typography,
  List,
  Box,
  Divider,
  CircularProgress,
  Alert,
  Paper,
} from "@mui/material";
import { ExportedFunctionInvokedEntry } from "@/types/api";

export default function InvocationLogs({
  lastClearTimeStamp,
}: {
  lastClearTimeStamp: Date | null;
}) {
  const { compId } = useParams<{ compId: string }>();
  const { id: workerName } = useParams<{ id: string }>();
  const { logs, error, isLoading } = useWorkerLogs(compId, workerName, {
    count: 100,
    query: "exported-function-invoked",
  });

  console.log(logs, error, isLoading);

  const checkLogIsAfterLastClearTime = useCallback(
    ({ entry }: { entry: ExportedFunctionInvokedEntry }) => {
      console.log("entering this");
      if (!lastClearTimeStamp) {
        return true;
      }

      const entryTimestamp = new Date(entry.timestamp);

      return entryTimestamp > lastClearTimeStamp;
    },
    [lastClearTimeStamp]
  );

  const entries = useMemo(() => {
    if (!logs) {
      return [];
    }
    const _entries = Array.isArray(logs?.entries) ? logs.entries : [];
    return _entries.filter(checkLogIsAfterLastClearTime) || [];
  }, [checkLogIsAfterLastClearTime, logs, lastClearTimeStamp]);

  if (isLoading)
    return (
      <Box
        display="flex"
        justifyContent="center"
        alignItems="center"
        height="100vh"
      >
        <CircularProgress />
      </Box>
    );

  if (error)
    return (
      <Box
        display="flex"
        justifyContent="center"
        alignItems="center"
        height="100vh"
      >
        <Alert severity="error">Error: {error}</Alert>
      </Box>
    );

  if (!entries || entries.length === 0)
    return (
      <Box
        display="flex"
        justifyContent="center"
        alignItems="center"
        height="100vh"
      >
        <Typography>No entries available.</Typography>
      </Box>
    );

  return (
    <Box>
      <Paper elevation={3} sx={{ px: 2 }}>
        <List>
          {entries.map(
            (
              { entry }: { entry: ExportedFunctionInvokedEntry },
              index: number
            ) => (
              <>
                {index > 0 && <Divider sx={{ my: 1 }} color="" />}
                <Typography variant="h6" gutterBottom>
                  {new Date(entry?.timestamp).toLocaleString()}{" "}
                  {entry?.function_name}
                </Typography>
              </>
            )
          )}
        </List>
      </Paper>
    </Box>
  );
}
