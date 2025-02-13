import React from "react";
import { useWorkerLogs } from "@/lib/hooks/use-worker";
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
import { useCustomParam } from "@/lib/hooks/use-custom-param";

type WorkerId = {
  componentId: string;
  workerName: string;
};

type Plugin = {
  installation_id: string;
  plugin_name: string;
  plugin_version: string;
  parameters: Record<string, string>;
};

type LogEntry = {
  type: string;
  timestamp: string;
  worker_id?: WorkerId;
  component_version?: number;
  args?: string[];
  env?: Record<string, string>;
  account_id?: string;
  parent?: WorkerId;
  component_size?: number;
  initial_total_linear_memory_size?: number;
  initial_active_plugins?: Plugin[];
};

type LogItem = {
  oplogIndex: number;
  entry?: LogEntry;
};

type LogResponse = {
  entries?: LogItem[];
  next?: {
    next_oplog_index: number;
    current_component_version: number;
  };
  firstIndexInChunk?: number;
  lastIndex?: number;
};

export default function WorkerLogs() {
  const { compId } = useCustomParam();
  const { id: workerName } = useCustomParam();
  const { logs, error, isLoading } = useWorkerLogs<LogResponse>(compId, workerName, {
    count: 10,
  });

  if (isLoading) {
    return (
      <Box display="flex" justifyContent="center" alignItems="center" height="100vh">
        <CircularProgress />
      </Box>
    );
  }

  if (error) {
    return (
      <Box display="flex" justifyContent="center" alignItems="center" height="100vh">
        <Alert severity="error">Error: {error.toString()}</Alert>
      </Box>
    );
  }

  if (!logs?.entries || logs.entries.length === 0) {
    return (
      <Box display="flex" justifyContent="center" alignItems="center" height="100vh">
        <Typography>No logs available.</Typography>
      </Box>
    );
  }

  return (
    <Box>
      <List>
        {logs.entries.map((logItem) => {
          // Skip rendering if entry is missing
          if (!logItem?.entry) return null;

          const entry = logItem.entry;

          return (
            <div
              key={logItem.oplogIndex}
              variant="outlined"
              sx={{ mb: 2, p: 2, borderRadius: 2, boxShadow: 1 }}
            >
              <Typography variant="h6" gutterBottom>
                {entry.type ?? 'Unknown Type'} - {entry.timestamp ? new Date(entry.timestamp).toLocaleString() : 'No timestamp'}
              </Typography>

              <Divider sx={{ my: 1 }} />

              {entry.worker_id && (
                <>
                  <Typography variant="subtitle1">Worker Info:</Typography>
                  <List dense>
                    {entry.worker_id.componentId && (
                      <ListItem>Component ID: {entry.worker_id.componentId}</ListItem>
                    )}
                    {entry.worker_id.workerName && (
                      <ListItem>Worker Name: {entry.worker_id.workerName}</ListItem>
                    )}
                    {entry.component_version !== undefined && (
                      <ListItem>Component Version: {entry.component_version}</ListItem>
                    )}
                    {entry.component_size !== undefined && (
                      <ListItem>Component Size: {entry.component_size}</ListItem>
                    )}
                  </List>
                </>
              )}

              {entry.args && entry.args.length > 0 && (
                <>
                  <Typography variant="subtitle1" sx={{ mt: 2 }}>
                    Arguments:
                  </Typography>
                  <List dense>
                    {entry.args.map((arg, index) => (
                      <ListItem key={index}>{arg}</ListItem>
                    ))}
                  </List>
                </>
              )}

              {entry.env && Object.keys(entry.env).length > 0 && (
                <>
                  <Typography variant="subtitle1" sx={{ mt: 2 }}>
                    Environment Variables:
                  </Typography>
                  <List dense>
                    {Object.entries(entry.env).map(([key, value]) => (
                      <ListItem key={key}>
                        {key}: {value}
                      </ListItem>
                    ))}
                  </List>
                </>
              )}

              {entry.account_id && (
                <>
                  <Typography variant="subtitle1" sx={{ mt: 2 }}>
                    Account ID:
                  </Typography>
                  <Typography>{entry.account_id}</Typography>
                </>
              )}

              {entry.parent && (
                <>
                  <Typography variant="subtitle1" sx={{ mt: 2 }}>
                    Parent Info:
                  </Typography>
                  <List dense>
                    {entry.parent.componentId && (
                      <ListItem>Component ID: {entry.parent.componentId}</ListItem>
                    )}
                    {entry.parent.workerName && (
                      <ListItem>Worker Name: {entry.parent.workerName}</ListItem>
                    )}
                  </List>
                </>
              )}

              {entry.initial_total_linear_memory_size !== undefined && (
                <>
                  <Typography variant="subtitle1" sx={{ mt: 2 }}>
                    Memory Info:
                  </Typography>
                  <Typography>
                    Initial Total Linear Memory Size: {entry.initial_total_linear_memory_size}
                  </Typography>
                </>
              )}

              {entry.initial_active_plugins && entry.initial_active_plugins.length > 0 && (
                <>
                  <Typography variant="subtitle1" sx={{ mt: 2 }}>
                    Active Plugins:
                  </Typography>
                  <List dense>
                    {entry.initial_active_plugins.map((plugin, idx) => (
                      <Box key={plugin.installation_id || idx} sx={{ mb: 2 }}>
                        {plugin.plugin_name && (
                          <Typography>Plugin Name: {plugin.plugin_name}</Typography>
                        )}
                        {plugin.plugin_version && (
                          <Typography>Plugin Version: {plugin.plugin_version}</Typography>
                        )}
                        {plugin.installation_id && (
                          <Typography>Installation ID: {plugin.installation_id}</Typography>
                        )}
                        {plugin.parameters && Object.keys(plugin.parameters).length > 0 && (
                          <>
                            <Typography>Parameters:</Typography>
                            <List dense>
                              {Object.entries(plugin.parameters).map(([key, value]) => (
                                <ListItem key={key}>
                                  {key}: {value}
                                </ListItem>
                              ))}
                            </List>
                          </>
                        )}
                      </Box>
                    ))}
                  </List>
                </>
              )}
            </div>
          );
        })}
      </List>

      {logs.next && (
        <Box sx={{ mt: 2 }}>
          <Typography variant="body2" color="text.secondary">
            Next Index: {logs.next.next_oplog_index}
          </Typography>
          <Typography variant="body2" color="text.secondary">
            Current Component Version: {logs.next.current_component_version}
          </Typography>
        </Box>
      )}
    </Box>
  );
}