/* eslint-disable @typescript-eslint/ban-ts-comment */
import React, { useState } from "react";
import {
  Box,
  Button,
  Divider,
  FormControl,
  IconButton,
  MenuItem,
  OutlinedInput,
  Select,
  TextField,
  Typography,
  Paper
} from "@mui/material";
import AddIcon from "@mui/icons-material/Add";
import RemoveIcon from "@mui/icons-material/Remove";
import { useForm, Controller, useFieldArray } from "react-hook-form";
import useComponents from "@/lib/hooks/use-component";
import { Component, WorkerFormData } from "@/types/api";
import { v4 as uuidv4 } from 'uuid';
import { addNewWorker } from "@/lib/hooks/use-worker";
import {getFormErrorMessage} from "@/lib/utils"

interface FormData {
  component: string;
  workerName: string;
  arguments: { value: string }[];
  envVars: { key: string; value: string }[];
}

const CreateWorker = ({compId, version, onSuccess}:{compId?:string, version?:string|number, onSuccess?:()=>void}) => {
  const {
    control,
    handleSubmit,
    formState: { errors },
    setValue
  } = useForm<FormData>({
    defaultValues: {
      envVars: [{ key: "", value: "" }],
      arguments:[{value:""}],
      workerName:"",
      component:""
    },
  });

  const { fields, append, remove } = useFieldArray({
    control,
    name: "envVars",
  });

  const { fields: argumentFields, append: appendArgument, remove: removeArgument } = useFieldArray({
    control,
    name: "arguments",
  });
  
  const [error, setError] = useState("");
  const { components } = useComponents(compId, version);

  const addEnvVar = () => {
    append({ key: "", value: "" });
  };
  const addArgument = () => {
    appendArgument({ value: "" });
  };

  const removeEnvVar = (index: number) => {
    if (index === 0) return;
    remove(index);
  };

 
const removeArgumentVar = (index: number) => {
  removeArgument(index);
};
  const onSubmit = async (data: FormData) => {
    console.log("data:", data);

    const newWorker = {
      name: data.workerName,
      args:data.arguments?.map((arg)=>arg.value).filter(val=>!!val),
      env: data.envVars.reduce<Record<string,string>>((acc, { key, value }) => {
        if (key && value) acc[key] = value;
        return acc;
      }, {}),
    } as WorkerFormData;
    const {error} = await addNewWorker(newWorker, (data.component|| compId!));
    setError(error || "");
    if(error) {
     return
    }
    onSuccess?.()

  };

  return (
    <Paper
      component="form"
      elevation={4}
      onSubmit={handleSubmit(onSubmit)}
      sx={{
        width: "100%",
        p: 3,
      }}
    >
      <Box sx={{ display: "flex", gap: 2}}>
        <FormControl fullWidth>
          <Typography variant="body2" sx={{ mb: 1 }}>
            Component
          </Typography>
          <Controller
            name="component"
            control={control}
            rules={{required: 'Component is required!'}}
            render={({ field }) => (
              <Select {...field}>
                {components?.map((component: Component) => (
                  <MenuItem
                    key={component.versionedComponentId.componentId}
                    value={component.versionedComponentId.componentId}
                  >
                    {component.componentName}
                  </MenuItem>
                ))}
              </Select>
            )}
          />
        </FormControl>
      </Box>
      <Typography variant="caption" color="error">{getFormErrorMessage("component", errors)}</Typography>{}

      {/* Worker Name */}
      <Box sx={{ display: "flex", alignItems: "center", gap: 2, mt: 3 }}>
        <Controller
          name="workerName"
          control={control}
          rules={{ required: "Worker Name is required" }}
          render={({ field }) => (
            <TextField
              {...field}
              fullWidth
              label="Worker Name"
              variant="outlined"
            />
          )}
        />
        <Button
          variant="contained"
          sx={{ textTransform: "none" }}
          onClick={(e) => {
            e.preventDefault();
            setValue("workerName", uuidv4());
          }}
        >
          Generate
        </Button>
      </Box>
      <Typography variant="caption" color="error">{getFormErrorMessage("workerName", errors)}</Typography>{}
      {/* Environment Variables */}
      <Box sx={{ mb: 3 }}>
        <Box
          sx={{
            display: "flex",
            justifyContent: "space-between",
            alignItems: "center",
            gap: 2,
            mb: 2,
          }}
        >
          <Typography variant="body2" sx={{ mb: 1 }}>
            Environment Variables
          </Typography>
          <Button
            startIcon={<AddIcon />}
            variant="outlined"
            sx={{ textTransform: "none" }}
            onClick={addEnvVar}
          >
            Add
          </Button>
        </Box>
        {fields.map((item, index) => (
          <Box
            key={item.id}
            sx={{ display: "flex", alignItems: "center", gap: 2, mb: 2 }}
          >
            <Controller
              //@ts-ignore
              name={`envVars[${index}].key`}
              control={control}
              render={({ field }) => (
                <TextField
                  {...field}
                  fullWidth
                  label="Key"
                  variant="outlined"
                />
              )}
            />
            <Controller
              //@ts-ignore
              name={`envVars[${index}].value`}
              control={control}
              render={({ field }) => (
                <OutlinedInput {...field} fullWidth type="password" />
              )}
            />
            <IconButton onClick={() => removeEnvVar(index)}>
              <RemoveIcon sx={{ bgcolor: "#71bdf6", borderRadius: "50%" }} />
            </IconButton>
          </Box>
        ))}
      </Box>

      <Divider sx={{ backgroundColor: "#333", mb: 3 }} />

      {/* Arguments */}
      <Box sx={{ mb: 3 }}>
        <Box
          sx={{
            display: "flex",
            justifyContent: "space-between",
            alignItems: "center",
            gap: 2,
            mb: 2,
          }}
        >
          <Typography variant="body2" sx={{ mb: 1 }}>
            Arguments
          </Typography>
          <Button
            startIcon={<AddIcon />}
            variant="outlined"
            sx={{ textTransform: "none" }}
            onClick={addArgument}
          >
            Add
          </Button>
        </Box>
        {argumentFields.map((item, index) => (
          <Box
            key={item.id}
            sx={{ display: "flex", alignItems: "center", gap: 2, mb: 2 }}
          >
            <Controller
              //@ts-ignore
              name={`arguments.${index}.value`} 
              control={control}
              render={({ field }) => (
                <TextField
                  {...field}
                  fullWidth
                  label={`Argument ${index + 1}`}
                  variant="outlined"
                />
              )}
            />
            <IconButton onClick={() => removeArgumentVar(index)}>
              <RemoveIcon sx={{ bgcolor: "#71bdf6", borderRadius: "50%" }} />
            </IconButton>
          </Box>
        ))}
      </Box>
      {error && <Typography variant="caption" color="error">{error}</Typography>}  
      {/* Create Worker Button */}
      <Box sx={{ textAlign: "center" }}>
        <Button
          type="submit"
          variant="contained"
          sx={{
            textTransform: "none",
            width: "100%",
            py: 1.5,
            backgroundColor: "#1976d2",
            ":hover": { backgroundColor: "#125ea5" },
          }}
        >
          Create Worker
        </Button>
      </Box>
    </Paper>
  );
};

export default CreateWorker;
