import React, { useCallback, useMemo } from "react";
import {
  Typography,
  List,
  Box,
  Divider,
  Paper,
  Stack,
} from "@mui/material";
import { EventMessage, StdOutMessage } from "@/types/api";

export default function TerminalLogs({
  lastClearTimeStamp,
  messages,
}: {
  lastClearTimeStamp: Date | null;
  messages: Array<EventMessage>;
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
    ) as StdOutMessage[];
  }, [checkLogIsAfterLastClearTime, messages]);

  if (!entries || entries.length === 0)
    return (
      <Box
        display="flex"
        justifyContent="center"
        alignItems="center"
        minHeight="100vh"
      >
        <Typography>No entries available.</Typography>
      </Box>
    );

  return (
    <Box>
        <List>
          {entries.map((entry:StdOutMessage , index: number) => (
            <Stack key={index}>
              {index > 0 && <Divider className="my-1 bg-border"/>}
              <Typography variant="body2" sx={{fontFamily:'monospace'}}>
                {new Date(entry?.StdOut?.timestamp).toLocaleString()}{" "}
                {entry?.StdOut?.bytes &&
                  String.fromCharCode(...entry?.StdOut?.bytes)}
              </Typography>
            </Stack>
          ))}
        </List>
    </Box>
  );
}
