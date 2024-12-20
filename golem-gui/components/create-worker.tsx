import React from "react";
import { useForm, Controller } from "react-hook-form";
import { WorkerFormData } from "@/types/api"; 
import {
  TextField,
  Button,
  Box,
  MenuItem,
  Typography,
  Alert,
} from "@mui/material";

const CreateWorker = () => {
  const {
    handleSubmit,
    control,
    formState: { errors },
    reset,
  } = useForm<WorkerFormData>();

  const workerTypes = [
    { label: "Type A", value: "type-a" },
    { label: "Type B", value: "type-b" },
    { label: "Type C", value: "type-c" },
  ];

  const onSubmit=(data: WorkerFormData) => {
    console.log("Worker Created:", data);
    // Logic to handle worker creation
    reset(); // Reset form after submission
  };

  return (
    <Box
      sx={{
        p: 3,
        backgroundColor: "#1E1E1E",
        color: "#FFFFFF",
        borderRadius: 2,
        boxShadow: 3,
        width: "100%",
        maxWidth: 500,
        margin: "0 auto",
      }}
    >
      <Typography variant="h5" gutterBottom>
        Create a New Worker
      </Typography>
      <Typography variant="body2" sx={{ mb: 2 }}>
        Fill out the details below to create a worker.
      </Typography>
      <form onSubmit={handleSubmit(onSubmit)}>
        {/* Worker Name */}
        <Box sx={{ mb: 3 }}>
          <Controller
            name="workerName"
            control={control}
            defaultValue=""
            rules={{ required: "Worker name is required" }}
            render={({ field }) => (
              <TextField
                {...field}
                label="Worker Name"
                variant="outlined"
                fullWidth
                sx={{ backgroundColor: "#2C2C2C" }}
                InputLabelProps={{ style: { color: "#999" } }}
                error={!!errors.workerName}
                // helperText={errors.workerName?.message}
              />
            )}
          />
        </Box>

        {/* Worker Type */}
        <Box sx={{ mb: 3 }}>
          <Controller
            name="workerType"
            control={control}
            defaultValue=""
            rules={{ required: "Worker type is required" }}
            render={({ field }) => (
              <TextField
                {...field}
                label="Worker Type"
                variant="outlined"
                select
                fullWidth
                sx={{ backgroundColor: "#2C2C2C" }}
                InputLabelProps={{ style: { color: "#999" } }}
                error={!!errors.workerType}
            //@ts-ignore
                helperText={errors.workerType?.message}
              >
                {workerTypes.map((type) => (
                  <MenuItem key={type.value} value={type.value}>
                    {type.label}
                  </MenuItem>
                ))}
              </TextField>
            )}
          />
        </Box>

        {/* Error Alert */}
        

        {/* {errors.formError && (
          <Alert severity="error" sx={{ mb: 2 }}>
            {
            errors.formError.message}
          </Alert>
        )} */}

        {/* Submit Button */}
        <Button
          type="submit"
          variant="contained"
          fullWidth
          sx={{
            backgroundColor: "#1976D2",
            textTransform: "none",
            ":hover": { backgroundColor: "#125EA5" },
          }}
        >
          Create Worker
        </Button>
      </form>
    </Box>
  );
};

export default CreateWorker;
