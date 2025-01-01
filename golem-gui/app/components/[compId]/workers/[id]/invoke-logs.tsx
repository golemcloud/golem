import React, { useCallback, useMemo } from "react";
import {
  Typography,
  List,
  Box,
  Divider,
  Paper,
} from "@mui/material";
import { EventMessage, InvocationStart } from "@/types/api";

type InovkeMeta = {
  status: "Pending" | "Finished",
  startTime: string
  endTime: string
}


export default function InvocationLogs({
  lastClearTimeStamp,
  messages,
}: {
  lastClearTimeStamp: Date | null;
  messages: Array<EventMessage>;
}) {
  const checkLogIsAfterLastClearTime = useCallback(
    (entryTime: string) => {
      console.log("entering this");
      if (!lastClearTimeStamp) {
        return true;
      }

      if (!entryTime) {
        return false;
      }

      const entryTimestamp = new Date(entryTime);

      return entryTimestamp > lastClearTimeStamp;
    },
    [lastClearTimeStamp]
  );

  console.log("messages in invokvelogs", messages);
  const invokeMessages = useMemo(() => {
    return Object.values(
      messages?.reduce<Record<string, InvocationStart["InvocationStart"]&InovkeMeta>>((obj, message: EventMessage) => {
       
        if ("InvocationStart" in message) {
          const idempotency_key = message?.["InvocationStart"]?.idempotency_key;
          const isEligible =
            idempotency_key &&
            checkLogIsAfterLastClearTime(message?.["InvocationStart"].timestamp);
          if(isEligible) {
          obj[idempotency_key] = {
            ...obj[idempotency_key],
            startTime: message["InvocationStart"].timestamp,
            status:  obj[idempotency_key]?.status || "Pending",
          };
          }else {
            delete obj[idempotency_key]
          }
        }

        if ("InvocationFinished" in message) {
          const idempotency_key = message?.["InvocationFinished"]?.idempotency_key;
          obj[idempotency_key] = {
            ...obj[idempotency_key],
            ...message["InvocationFinished"],
            endTime: message["InvocationFinished"].timestamp,
            status: "Finished",
          };
        }
        return obj;
      }, {}) || {}
    );
  }, [messages, checkLogIsAfterLastClearTime]);
  console.log(invokeMessages);

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

  if (!invokeMessages || invokeMessages.length === 0)
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
          {invokeMessages.map((entry:InvocationStart["InvocationStart"]&InovkeMeta , index: number) => (
            entry?.startTime ? <>
              {index > 0 && <Divider sx={{ my: 1 }} color="" />}
              <Typography variant="h6" gutterBottom>
                {new Date(entry?.startTime).toLocaleString()} {entry?.function}{" "}
                <span className="px-4 py-1 border">{entry.status}</span>
                {"  "}
                {entry.status === "Finished"
                  ? `${
                      new Date(entry.endTime).getTime() -
                      new Date(entry.startTime).getTime()
                    } ms`
                  : ""}{" "}
              </Typography>
            </>: null
          ))}
        </List>
      </Paper>
    </Box>
  );
}
