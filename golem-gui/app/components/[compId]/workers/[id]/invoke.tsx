import React, { useMemo, useState } from "react";
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
} from "@mui/material";
import { fetcher, getErrorMessage } from "@/lib/utils";
import DynamicForm from "./form-generator";

export function InvokeForm({
  invoke,
}: {
  invoke: { fun?: WorkerFunction; instanceName?: string };
}) {
  const { compId, id: workerName } = useParams<{
    compId: string;
    id: string;
  }>();
  // const { control, handleSubmit } = useForm();
  const [error, setError] = useState<string | null>(null);

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const onSubmit = async (data: any) => {
    try {
      //still WIP.
      const response = await fetcher(
        `?path=components/${compId}/workers/${workerName}/invoke-and-await?function=${invoke.instanceName}.{${invoke?.fun?.name}}`,
        {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({params: invoke?.fun?.parameters || []}),
        });

      console.log("response====>1234", response);
      if (response.status !== 200) {
        return setError(getErrorMessage(response.data));
      }
    } catch (err) {
      console.log("error", err);
    }
  };

  const paramsConfig = [...(invoke?.fun?.parameters||[]), 
  //sample tuple field config.
  {
    "name": "test",
    "typ": {
      "type": "Tuple",
      "items": [
        {
          "name":"item",
          "typ":{
          "fields": [
            {
              "name": "product-id",
              "typ": {
                "type": "Str"
              }
            },
            {
              "name": "name",
              "typ": {
                "type": "Str"
              }
            },
            {
              "name": "price",
              "typ": {
                "type": "F32"
              }
            },
            {
              "name": "quantity",
              "typ": {
                "type": "U32"
              }
            }
          ],
          "type": "Record"
        }
        },
        {
          name: "name",
          typ: {
            type: "Str",
          },
        }
      ]
    }
  }]

  return (
    <>
      <Typography variant="h6" gutterBottom>
        {invoke?.fun?.name}
      </Typography>
      {error && <Typography className="text-red-500 text-sm">{error}</Typography>}
      <DynamicForm config={paramsConfig} onSubmit={onSubmit}/>
    </>
  );
}

export default function InvokePage() {
  const { compId } = useParams<{
    compId: string;
  }>();
  const { components, isLoading } = useComponents(compId, "latest");
  const [latestComponent] = components;
  const [invoke, setInvoke] = useState<{
    fun?: WorkerFunction;
    instanceName?: string;
  } | null>(null);
  const exports = useMemo(() => {
    const exports = latestComponent?.metadata?.exports || [];
    setInvoke(
      exports[0]
        ? { fun: exports[0]?.functions?.[0], instanceName: exports[0]?.name }
        : null
    );
    return exports;
  }, [latestComponent?.metadata?.exports]);

  if (isLoading) {
    return <Loader />;
  }

  return (
    <Grid container spacing={4} columns={12} marginTop={4}>
      {/* Exports Section */}
      <Grid xs={4}>
        <Paper sx={{ padding: 3, bgcolor: "#1E1E1E" }}>
          <Typography variant="h6">Exports</Typography>
          <Divider sx={{ bgcolor: "#424242", marginY: 1 }} />
          <List>
            {exports.map((item, index) => (
              <Stack key={index}>
                <Typography>{item.name}</Typography>
                <ListItem disableGutters>
                  <List sx={{ marginLeft: 2 }}>
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
                            marginBottom: "0.8rem",
                            cursor: "pointer",
                            borderRadius: "10px",
                            backgroundColor: isActive
                              ? "#373737"
                              : "transparent",
                            "&:hover": { backgroundColor: "#373737" },
                          }}
                          className={`dark:hover:bg-[#373737] hover:bg-[#C0C0C0] ${
                            isActive
                              ? "dark:bg-[#373737] bg-[#C0C0C0]"
                              : "transparent"
                          }`}
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
      <Grid xs={8}>
        <Paper sx={{ padding: 3 }}>
          {invoke ? (
            //TODOD: basic creation of form with validations were implemented to integrate with backend. need lots of improvement on stylinng part
            <InvokeForm invoke={invoke}/>
          ) : (
            <Typography variant="body1">
              Select a function to invoke.
            </Typography>
          )}
        </Paper>
      </Grid>
    </Grid>
  );
}
