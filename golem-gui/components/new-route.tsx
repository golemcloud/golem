/* eslint-disable @typescript-eslint/no-explicit-any */
"use client";

import React, { useState } from "react";
import { useForm, Controller } from "react-hook-form";
import {
  Box,
  Button,
  TextField,
  Typography,
  Select,
  MenuItem,
  InputLabel,
  FormControl,
  Divider,
} from "@mui/material";
import useSWR from "swr";
import { fetcher } from "@/lib/utils";
import { ApiRoute, Component } from "@/types/api";
import { Loader } from "lucide-react";
import useApiDefinitions from "@/lib/hooks/use-api-definitons";
import { DeleteForever } from "@mui/icons-material";

type FormData = {
  path: string;
  workerName: string;
  response: string;
  method: string;
  component: string;
  version: string;
};

const NewRouteForm = ({
  apiId,
  version,
  isModal,
  isExperimental,
  defaultRoute,
  onSuccess
}: {
  apiId: string;
  version?: string;
  onSuccess?: () => void;
  isModal?: boolean;
  isExperimental?: boolean;
  defaultRoute?: ApiRoute | null
}) => {

  const { control, handleSubmit, reset, watch,  formState: { errors }, } = useForm<FormData>({
    defaultValues: {
      path: defaultRoute?.path || "",
      workerName: defaultRoute?.binding?.workerName || "",
      response: defaultRoute?.binding?.response || "",
      method: defaultRoute?.method || "Get",
      component: defaultRoute?.binding?.componentId.componentId || "",
      version: defaultRoute?.binding?.componentId.version?.toString() || "",
    },
  });

  const component = watch("component");
  const [error, setError] = useState<string | null>(null);
  const { data, isLoading } = useSWR("?path=components", fetcher);
  const { getApiDefintion, isLoading: apiDefinitonLoading, upsertRoute, deleteRoute } = useApiDefinitions(apiId, version);
  const { error: apiDefintionError } = getApiDefintion(apiId, version);
  const components = (data?.data || null) as Component[];

  if (apiDefinitonLoading || isLoading) {
    return <Loader />;
  }

  const versionNotFound = !apiDefinitonLoading && apiDefintionError
  const onSubmit = async (formData: any) => {
    if (isExperimental) {
      return;
    }
    try {
      const newRoute: ApiRoute = {
        path: formData.path,
        method: formData.method,
        security: null,
        binding: {
          bindingType: "default",
          componentId: {
            componentId: formData.component,
            version: Number(formData.version),
          },
          corsPreflight: null,
          idempotencyKey: null,
          idempotencyKeyInput: null,
          response: formData.response,
          responseMappingInput: {
            types: {},
          },
          responseMappingOutput: {},
          workerName: formData.workerName,
          workerNameInput: {
            types: {},
          },
        },
      };
      const { success, error } = await upsertRoute(apiId, newRoute, version)

      if (!success) {
        return setError(error!);
      }
      onSuccess?.();
    } catch (error) {
      console.error("Error creating route:", error);
    }
  };

  const handleDelete = async () => {
     await deleteRoute(defaultRoute!);
    onSuccess?.();
  };

  return (
    <Box
      sx={{
        width: isModal ? "50%" : "100%",
        margin: "auto",
        padding: 10,
      }}
    >
      {isExperimental && (
        <Typography
          variant="h5"
          fontWeight="bold"
          mb={2}
          className="text-red-500 text-center"
        >
          Experimental. Coming soon!
        </Typography>
      )}

      {/* Title */}
      <Box className="flex justify-between">
        <Typography variant="h5" gutterBottom>
          {defaultRoute ? "Update" : "New"} Route
        </Typography>
        {defaultRoute && <Button
          onClick={handleDelete}
        >
          <DeleteForever />
        </Button>}
      </Box>

      <Divider sx={{ borderColor: "#555" }} />

      {/* HTTP Endpoint */}
      <Box sx={{ marginTop: 4 }}>
        <Typography variant="subtitle1" gutterBottom>
          HTTP Endpoint
        </Typography>
        <Typography variant="body2" sx={{ color: "#AAA" }}>
          Each API Route must have a unique Method + Path combination
        </Typography>
        <Box className="my-5">
          {[
            "Get",
            "Post",
            "Put",
            "Patch",
            "Delete",
            "Head",
            "Options",
            "Trace",
            "Connect",
          ].map((method) => (
            <Controller
              key={method}
              name="method"
              control={control}
              rules={{ required: "Method is mandatory!" }}
              render={({ field }) => (
                <Button
                  {...field}
                  onClick={() => field.onChange(method)}
                  variant="outlined"
                  sx={{
                    textTransform: "none",
                    padding: "4px 10px",
                    marginInline: "5px",
                    borderRadius: "8px",
                    backgroundColor:
                      watch("method") === method ? "#696969" : "transparent",
                    color: watch("method") === method ? "#FFF" : "#000000",
                    borderColor: "#555",
                    "&:hover": {
                      backgroundColor: "#555",
                      color: "#FFF",
                    },
                  }}
                  className="dark:text-white"
                >
                  {method}
                </Button>
              )}
            />
          ))}
        </Box>
        {errors && errors.method && <Typography variant="inherit" color="error">{errors.method.message?.toString()}</Typography>}
        <Controller
          name="path"
          control={control}
          //we can add regex for path
          rules={{ required: "path is mandatory!" }}
          render={({ field }) => (
            <TextField
              {...field}
              fullWidth
              variant="outlined"
              label="Path"
              placeholder="/api/endpoint/{VARIABLE_NAME}"
            />
          )}
        />
        {errors && errors.path && <Typography variant="inherit" color="error">{errors.path.message?.toString()}</Typography>}
      </Box>

      {/* Worker Binding */}
      <Box sx={{ marginTop: 4 }}>
        <Typography variant="subtitle1" gutterBottom>
          Worker Binding
        </Typography>
        <Box sx={{ display: "flex", gap: 2, marginTop: 2 }}>
          <FormControl fullWidth>
            <InputLabel sx={{ color: "#AAA" }}>Component</InputLabel>
            <Controller
              name="component"
              control={control}
              rules={{ required: "Component is mandatory!" }}
              render={({ field }) => (
                <Select
                  {...field}
                  variant="outlined"
                  label="Component"
                  disabled={isLoading || components?.length == 0}
                >
                  {components?.map((component: Component) => (
                    <MenuItem
                      key={component?.versionedComponentId?.componentId}
                      value={component?.versionedComponentId?.componentId}
                    >
                      {component.componentName}
                    </MenuItem>
                  ))}
                </Select>
              )}
            />
            {errors && errors.component && <Typography variant="inherit" color="error">{errors.component.message?.toString()}</Typography>}
          </FormControl>

          <FormControl fullWidth>
            <InputLabel sx={{ color: "#AAA" }}>Version</InputLabel>
            <Controller
              name="version"
              control={control}
              rules={{ required: "Component is mandatory!" }}
              render={({ field }) => (
                <Select
                  {...field}
                  variant="outlined"
                  label="Version"
                  disabled={isLoading || components?.length == 0}
                >
                  {!isLoading &&
                    components?.map((comp: Component) => {
                      return comp?.versionedComponentId?.componentId ==
                        component ? (
                        <MenuItem
                          key={`${comp?.versionedComponentId?.componentId}__${comp.versionedComponentId.version}`}
                          value={comp.versionedComponentId.version}
                        >
                          {comp.versionedComponentId.version}
                        </MenuItem>
                      ) : null;
                    })}
                </Select>
              )}
            />
            {errors && errors.version && <Typography variant="inherit" color="error">{errors.version.message?.toString()}</Typography>}
          </FormControl>
        </Box>
        <Box sx={{ marginTop: 4 }}>
          <Typography variant="subtitle1" gutterBottom>
            Worker Name
          </Typography>
          <Controller
            name="workerName"
            control={control}
            // TODO: need to add the rib expression sysntax regex.
            rules={{ required: "workername is mandatory!" }}
            render={({ field }) => (
              <TextField
                {...field}
                fullWidth
                variant="outlined"
                label="Worker Name"
                placeholder="Worker name in Rib expression"
                multiline
              />
            )}
          />
          {errors && errors.workerName && <Typography variant="inherit" color="error">{errors.workerName.message?.toString()}</Typography>}
        </Box>
      </Box>

      {/* Response */}
      <Box sx={{ marginTop: 4 }}>
        <Typography variant="subtitle1" gutterBottom>
          Response
        </Typography>
        <Controller
          name="response"
          // TODO: need to add the rib expression sysntax regex.
          rules={{ required: "response is mandatory!" }}
          control={control}
          render={({ field }) => (
            <TextField
              {...field}
              fullWidth
              variant="outlined"
              label="Response"
              placeholder="Enter response in Rib expression"
              multiline
              rows={3}
              sx={{
                marginTop: 2,
              }}
            />
          )}
        />
        {errors && errors.response && <Typography variant="inherit" color="error">{errors.response.message?.toString()}</Typography>}
      </Box>
      {(versionNotFound || error) && (
        // TODO we need error type veraint as we are using it many places
        <Typography color="error">
          {versionNotFound || error}
        </Typography>
      )}
      {/* Buttons */}
      <Box
        sx={{ marginTop: 4, display: "flex", justifyContent: "space-between" }}
      >
        <Button
          variant="outlined"
          sx={{ color: "#FFF", borderColor: "#555" }}
          className="dark:text-[#FFF] dark:hover:bg-[#696969] text-gray-700 hover:bg-[#C0C0C0] dark:border-[#555]  "
          onClick={() => reset()}
        >
          Clear
        </Button>
        <Button
          variant="contained"
          color="primary"
          onClick={handleSubmit(onSubmit)}
          disabled={!!versionNotFound}
        >
          {defaultRoute ? 'Update' : 'Create'} Route
        </Button>
      </Box>
    </Box>
  );
};

export default NewRouteForm;
