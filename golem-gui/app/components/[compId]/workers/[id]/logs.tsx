import React from "react";
import { useWorkerLogs } from "@/lib/hooks/use-worker";
import { useParams } from "next/navigation";
import {
  Typography,
  List,
  ListItem,
  Box,
  Divider,
  CircularProgress,
  Alert,
  Paper,
} from "@mui/material";

export default function WorkerLogs() {
  const { compId } = useParams<{ compId: string }>();
  const { id: workerName } = useParams<{ id: string }>();
  const { logs, error, isLoading } = useWorkerLogs(compId, workerName, {
    count: 1,
  });

  console.log(logs, error, isLoading);

  const entries = logs?.entries || [];

  if (isLoading)
    return (
      <Box display="flex" justifyContent="center" alignItems="center" height="100vh">
        <CircularProgress />
      </Box>
    );

  if (error)
    return (
      <Box display="flex" justifyContent="center" alignItems="center" height="100vh">
        <Alert severity="error">Error: {error}</Alert>
      </Box>
    );

  if (!entries || entries.length === 0)
    return (
      <Box display="flex" justifyContent="center" alignItems="center" height="100vh">
        <Typography>No entries available.</Typography>
      </Box>
    );

  return (
    <Box p={3}>
      <List>
        {entries.map((log: any, index: number) => (
          <Paper
            key={index}
            variant="outlined"
            sx={{ mb: 2, p: 2, borderRadius: 2, boxShadow: 1 }}
          >
            <Typography variant="h6" gutterBottom>
              {log.entry.type} - {new Date(log.entry.timestamp).toLocaleString()}
            </Typography>

            <Divider sx={{ my: 1 }} />

            <Typography variant="subtitle1" >
              Worker Info:
            </Typography>
            <List dense>
              <ListItem>Component ID: {log.entry.worker_id.componentId}</ListItem>
              <ListItem>Worker Name: {log.entry.worker_id.workerName}</ListItem>
            </List>

            <Typography variant="subtitle1"  sx={{ mt: 2 }}>
              Environment Variables:
            </Typography>
            <List dense>
              {Object.entries(log.entry.env).map(([key, value]) => (
                <ListItem key={key}>
                  {key}: {String(value)}
                </ListItem>
              ))}
            </List>

            <Typography variant="subtitle1"  sx={{ mt: 2 }}>
              Account ID:
            </Typography>
            <Typography>{log.entry.account_id}</Typography>

            {log.entry.parent && (
              <>
                <Typography variant="subtitle1"  sx={{ mt: 2 }}>
                  Parent Info:
                </Typography>
                <List dense>
                  <ListItem>Parent Component ID: {log.entry.parent.componentId}</ListItem>
                  <ListItem>Parent Worker Name: {log.entry.parent.workerName}</ListItem>
                </List>
              </>
            )}

            <Typography variant="subtitle1" sx={{ mt: 2 }}>
              Initial Memory Size:
            </Typography>
            <Typography>{log.entry.initial_total_linear_memory_size}</Typography>

            <Typography variant="subtitle1" sx={{ mt: 2 }}>
              Active Plugins:
            </Typography>
            <List dense>
              {log.entry.initial_active_plugins.map((plugin: any, idx: number) => (
                <Box key={idx} sx={{ mb: 2 }}>
                  <Typography>Plugin Name: {plugin.plugin_name}</Typography>
                  <Typography>Plugin Version: {plugin.plugin_version}</Typography>
                  <Typography>Installation ID: {plugin.installation_id}</Typography>
                  <Typography>Parameters:</Typography>
                  <List dense>
                    {Object.entries(plugin.parameters).map(([paramKey, paramValue]) => (
                      <ListItem key={paramKey}>
                        {paramKey}: {String(paramValue)}
                      </ListItem>
                    ))}
                  </List>
                </Box>
              ))}
            </List>
          </Paper>
        ))}
      </List>
    </Box>
  );
}
