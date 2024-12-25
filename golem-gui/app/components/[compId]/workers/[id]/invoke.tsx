import React, { useEffect, useMemo, useState } from "react";
import { WorkerFunction } from "@/types/api";
import useComponents from "@/lib/hooks/use-component";
import { useParams } from "next/navigation";
import { Loader } from "lucide-react";
import {
  Paper,
  Typography,
  Divider,
  ListItem,
  ListItemText,
  List,
  Stack,
  Grid,
  Box,
} from "@mui/material";
import DynamicForm from "./form-generator";
import { useWorkerInvocation } from "@/lib/hooks/use-worker";

export function InvokeForm({
  invoke,
}: {
  invoke: { fun?: WorkerFunction; instanceName?: string };
}) {
  const { result, error, invokeFunction } = useWorkerInvocation(invoke);
  const paramsConfig = useMemo(() => invoke?.fun?.parameters || [], [invoke]);

  const onSubmit = async (data: any) => {
    invokeFunction(data);
  };

  return (
    <Box>
      <Typography variant="h5" fontWeight="bold" gutterBottom>
        {invoke?.fun?.name || "Invoke Function"}
      </Typography>
      {error && (
        <Typography variant="body2" color="error" sx={{ marginBottom: 2 }}>
          {error}
        </Typography>
      )}
      <DynamicForm config={paramsConfig} onSubmit={onSubmit} />
      {result && (
        <Box
          mt={2}
          p={1}
          bgcolor="#c0c0c0"
          borderRadius={2}
          overflow="auto"
          sx={{
            whiteSpace: "pre-wrap",
            fontFamily: "monospace",
            color: "black",
            fontSize: "0.9rem",
          }}
          className="dark:bg-[#555] dark:text-white"
          
        >
          <Typography variant="subtitle1" sx={{ fontWeight: "bold" }}>
            Result:
          </Typography>
          <Box
            component="pre"
            sx={{
              backgroundColor: "#f5f5f5",
              padding: "10px",
              borderRadius: "5px",
              overflowX: "auto",
              marginTop: "8px",
            }}
            className="dark:bg-[#1e1e1e] dark:text-[#f5f5f5]"
          >
            {JSON.stringify(result, null, 2)}
          </Box>
        </Box>
      )}
    </Box>
  );
}

export default function InvokePage() {
  const { compId } = useParams<{ compId: string }>();
  const { components, isLoading } = useComponents(compId, "latest");
  const [latestComponent] = components;
  const [invoke, setInvoke] = useState<{
    fun?: WorkerFunction;
    instanceName?: string;
  } | null>(null);

  const exports = useMemo(() => {
    const componentExports = latestComponent?.metadata?.exports || [];
    setInvoke(
      componentExports[0]
        ? { fun: componentExports[0]?.functions?.[0], instanceName: componentExports[0]?.name }
        : null
    );
    return componentExports;
  }, [latestComponent?.metadata?.exports]);

  if (isLoading) {
    return <Loader />;
  }

  return (
    <Grid container spacing={4} marginTop={4}>
      {/* Exports Section */}
      <Grid item xs={12} md={3}>
        <Paper
          sx={{
            padding: 3,
            bgcolor: "background.paper",
            boxShadow: 3,
            borderRadius: 2,
          }}
        >
          <Typography variant="h6" fontWeight="bold" gutterBottom>
            Exports
          </Typography>
          <Divider sx={{ marginY: 2,bgcolor:'#555' }} />
          <List>
            {exports.map((item, index) => (
              <Stack key={index} spacing={1}>
                <Typography variant="subtitle1" fontWeight="bold">
                  {item.name}
                </Typography>
                <ListItem disableGutters>
                  <List sx={{ paddingLeft: 2 }}>
                    {item.functions.map((fun) => {
                      const isActive = invoke?.fun?.name === fun.name;
                      return (
                        <ListItem
                          key={fun.name}
                          disableGutters
                          onClick={() =>
                            setInvoke({ fun: fun, instanceName: item.name })
                          }
                          sx={{
                            px: 2,
                            marginBottom: "0.2rem",
                            cursor: "pointer",
                            borderRadius: "10px",
                          }}
                          className={`dark:hover:bg-[#1e1e1e] hover:bg-[#C0C0C0]
                          ${isActive ? "dark:bg-[#1e1e1e] bg-[#C0C0C0]" : "transparent"}`}
                        >
                          <ListItemText primary={fun.name} />
                        </ListItem>
                      );
                    })}
                  </List>
                </ListItem>
              </Stack>
            ))}
          </List>
        </Paper>
      </Grid>

      {/* Form Section */}
      <Grid item xs={12} md={9}>
        <Paper
          sx={{
            padding: 3,
            boxShadow: 3,
            borderRadius: 2,
            bgcolor: "background.paper",
          }}
        >
          {invoke ? (
            <InvokeForm invoke={invoke} />
          ) : (
            <Typography variant="body1" color="textSecondary">
              Select a function to invoke.
            </Typography>
          )}
        </Paper>
      </Grid>
    </Grid>
  );
}
