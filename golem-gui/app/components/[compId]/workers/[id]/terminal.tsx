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
import { PublicOplogEntry_LogParameters } from "@/types/api";

export default function TerminalLogs({
  lastClearTimeStamp,
  messages,
}: {
  lastClearTimeStamp: Date | null;
  messages: Array<any>;
}) {
  //TODO: we can make useCllaback and useMemo a custom hook. so that we can see this across all tabs.
  const checkLogIsAfterLastClearTime = useCallback(
    (timestamp: string) => {
      console.log("entering this");
      if (!lastClearTimeStamp) {
        return true;
      }

      const entryTimestamp = new Date(timestamp);

      return entryTimestamp > lastClearTimeStamp;
    },
    [lastClearTimeStamp]
  );

  const entries = useMemo(() => {
    if (!messages) {
      return [];
    }
    const _entries = Array.isArray(messages) ? messages : [];

    return (
      _entries.filter(
        (entry) =>
          "StdOut" in entry &&
          checkLogIsAfterLastClearTime(entry?.StdOut?.timestamp)
      ) || []
    );
  }, [checkLogIsAfterLastClearTime, messages]);

  // if (isLoading)
  //   return (
  //     <Box
  //       display="flex"
  //       justifyContent="center"
  //       alignItems="center"
  //       height="100vh"
  //     >
  //       <CircularProgress />
  //     </Box>
  //   );

  // if (error)
  //   return (
  //     <Box
  //       display="flex"
  //       justifyContent="center"
  //       alignItems="center"
  //       height="100vh"
  //     >
  //       <Alert severity="error">Error: {error}</Alert>
  //     </Box>
  //   );

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
          {entries.map((entry, index: number) => (
            <>
              {index > 0 && <Divider sx={{ my: 1 }} color="" />}
              <Typography variant="h6" gutterBottom>
                {new Date(entry?.StdOut?.timestamp).toLocaleString()}{" "}
                {entry?.StdOut?.bytes &&
                  String.fromCharCode(...entry?.StdOut?.bytes)}
              </Typography>
            </>
          ))}
        </List>
      </Paper>
    </Box>
  );
}
