"use client";

import React from "react";
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
import { Component } from "@/types/api";
import { useParams, useSearchParams } from "next/navigation";

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

  //TODO: have to integrate the components data to route form.
  const { data, isLoading } = useSWR("/v1/components", fetcher);

  const onSubmit = async (formData: unknown) => {
    if (isExperimental) {
      return;
    }
    try {
      //   fetcher(`?path=api/definitions/${apiId}/${version}`, {method: "POST", headers:{
      //     "content-type": "application/json"
      //   },
      //   body: JSON.stringify({
      //     routes: []
      //   })
      // })
      console.log("Route created successfully:", formData);
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
                  disabled={isLoading || data?.data?.length == 0}
                >
                  {data?.data?.map((component: Component) => (
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
                  disabled={isLoading || data?.data?.length == 0}
                >
                  {!isLoading &&
                    data?.data?.map((comp: Component) => {
                      return comp?.versionedComponentId?.componentId ==
                        component ? (
                        <MenuItem
                          key={comp?.versionedComponentId?.componentId}
                          value={comp?.versionedComponentId?.componentId}
                        >
                          {comp.componentName}
                        </MenuItem>
                      ) : null;
                    })}
                </Select>
              )}
            />
          </FormControl>
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
              placeholder="Enter response"
              multiline
              rows={3}
              sx={{
                marginTop: 2,
              }}
            />
          )}
        />
      </Box>

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
        >
          Create Route
        </Button>
      </Box>
    </Box>
  );
};

export default NewRouteForm;
