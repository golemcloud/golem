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
import { ApiDefinition, Component } from "@/types/api";
import { useParams, useSearchParams } from "next/navigation";
import { Loader } from "lucide-react";
import { getErrorMessage } from "../lib/utils";

type FormData = {
  path: "";
  workerName: "";
  response: "";
  method: "Get";
  component: "";
  version: "";
};

const NewRouteForm = ({
  apiId,
  version,
  isModal,
  isExperimental,
}: {
  apiId: string;
  version?: string;
  onCreation?: () => void;
  isModal?: boolean;
  isExperimental?: boolean;
}) => {
  const { control, handleSubmit, reset, watch } = useForm({
    defaultValues: {
      apiName: "",
      apiVersion: "",
      path: "",
      workerName: "",
      response: "",
      method: "Get",
      component: "",
      version: "",
    },
  });

  const component = watch("component");
  const [error, setError] = useState<string | null>(null);
  const { data, isLoading } = useSWR("?path=components", fetcher);
  const { data: apiData, isLoading: apiDefinitonLoading } = useSWR(
    `?path=api/definitions?api-definition-id=${apiId}`,
    fetcher
  );
  const apiDefinitions = (
    Array.isArray(apiData?.data || []) ? apiData?.data || [] : [apiData?.data]
  ) as ApiDefinition[];
  const apiDefinition =
    apiDefinitions.find((api) => api.version === version) ||
    apiDefinitions[apiDefinitions.length - 1] ||
    null;
  const components = (data?.data || null) as Component[];

  if (apiDefinitonLoading || isLoading) {
    return <Loader />;
  }

  const versionNotFound =
    !apiDefinitonLoading && !apiDefinition
      ? "Api defintion not found. please check apiId and version are valid"
      : "";

  const onSubmit = async (formData: any) => {
    if (isExperimental) {
      return;
    }
    try {
      const newRoute = {
        path: formData.path,
        method: formData.method,
        security: null,
        binding: {
          bindingType: "default",
          componentId: {
            componentId: formData.component,
            version: formData.version,
          },
          corsPreflight: null,
          idempotencyKey: null,
          idempotencyKeyInput: null,
          response: formData.response,
          responseMappingInput: {
            types: {},
          },
          responseMappingOutput: {},
          workername: formData.workerName,
          workerNameInput: {
            types: {},
          },
        },
      };
      const response = await fetcher(
        `?path=api/definitions/${apiId}/${version}`,
        {
          method: "PUT",
          headers: {
            "content-type": "application/json",
          },
          body: JSON.stringify({
            ...apiDefinition,
            routes: [...(apiDefinition?.routes || []), newRoute],
          }),
        }
      );

      if (response.status !== 200) {
        return setError(getErrorMessage(response.data));
      }
      console.log("Route created successfully:", response.data);
    } catch (error) {
      console.error("Error creating route:", error);
    }
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
      <Box className="flex">
        <Typography variant="h5" gutterBottom>
          New Route
        </Typography>
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
        <Controller
          name="path"
          control={control}
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
          </FormControl>

          <FormControl fullWidth>
            <InputLabel sx={{ color: "#AAA" }}>Version</InputLabel>
            <Controller
              name="version"
              control={control}
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
          </FormControl>
        </Box>
        <Box sx={{ marginTop: 4 }}>
          <Typography variant="subtitle1" gutterBottom>
            Worker Name
          </Typography>
          <Controller
            name="workerName"
            control={control}
            render={({ field }) => (
              <TextField
                {...field}
                fullWidth
                variant="outlined"
                label="Worker Name"
                placeholder="Worker name in Rib expression"
              />
            )}
          />
        </Box>
      </Box>

      {/* Response */}
      <Box sx={{ marginTop: 4 }}>
        <Typography variant="subtitle1" gutterBottom>
          Response
        </Typography>
        <Controller
          name="response"
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
      </Box>
      {(versionNotFound || error) && (
        <Typography className="text-red-500 text-sm">
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
          Create Route
        </Button>
      </Box>
    </Box>
  );
};

export default NewRouteForm;
