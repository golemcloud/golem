import { Box, Button, Typography, TextField, RadioGroup, FormControlLabel, Radio, Paper, IconButton } from '@mui/material';
import CloudUploadIcon from '@mui/icons-material/CloudUpload';
import FolderIcon from '@mui/icons-material/Folder';
import DeleteIcon from '@mui/icons-material/Delete';
import { useState } from 'react';

export default function CreateComponentForm() {
    const [files, setFiles] = useState<File[]>([]);
    const [wasmFile, setWasmFile] = useState<File | null>(null);
  const [componentName, setComponentName] = useState('');
  const [type, setType] = useState('durable');

  const handleWasmUpload = (e: any) => {
    setWasmFile(e.target.files[0]);
  };

  const handleFilesUpload = (e: React.ChangeEvent<HTMLInputElement>) => {
    const uploadedFiles = Array.from(e.target.files || []) as File[];
    setFiles((prevFiles) => [...prevFiles, ...uploadedFiles]);
  };
  

  const handleFileDelete = (index: number) => {
    const updatedFiles = [...files];
    updatedFiles.splice(index, 1);
    setFiles(updatedFiles);
  };

  return (
    <Paper elevation={3} sx={{ p: 4, maxWidth: '600px', mx: 'auto', mt: 5, borderRadius: 2 }}>
      <Typography variant="h5" fontWeight="bold" gutterBottom>
        Create a New Component
      </Typography>

      {/* Project and Component Name */}
      <Box sx={{ mb: 3 }}>
        <TextField
          select
          label="Project"
          SelectProps={{ native: true }}
          variant="outlined"
          fullWidth
          sx={{ mb: 2 }}
        >
          <option>First</option>
          <option>Second</option>
        </TextField>
        <TextField
          label="Component Name"
          placeholder="Enter component name"
          variant="outlined"
          fullWidth
          value={componentName}
          onChange={(e) => setComponentName(e.target.value)}
        />
      </Box>

      {/* Type Section */}
      <Box sx={{ mb: 3 }}>
        <Typography variant="body1" fontWeight="medium">
          Type
        </Typography>
        <RadioGroup
          row
          value={type}
          onChange={(e) => setType(e.target.value)}
        >
          <FormControlLabel
            value="durable"
            control={<Radio />}
            label="Durable"
          />
          <FormControlLabel
            value="ephemeral"
            control={<Radio />}
            label="Ephemeral"
          />
        </RadioGroup>
      </Box>

      {/* WASM Binary Upload */}
      <Box sx={{ mb: 3 }}>
        <Typography variant="body1" fontWeight="medium" gutterBottom>
          WASM Binary
        </Typography>
        <Button
          variant="contained"
          component="label"
          startIcon={<CloudUploadIcon />}
          sx={{ mb: 1 }}
        >
          Upload WASM
          <input
            type="file"
            accept=".wasm"
            hidden
            onChange={handleWasmUpload}
          />
        </Button>
        {wasmFile && (
          <Typography variant="body2">{wasmFile.name}</Typography>
        )}
      </Box>

      {/* Initial Files Upload */}
      <Box sx={{ mb: 3 }}>
        <Typography variant="body1" fontWeight="medium" gutterBottom>
          Initial Files
        </Typography>
        <Button
          variant="contained"
          component="label"
          startIcon={<CloudUploadIcon />}
          sx={{ mb: 1 }}
        >
          Upload Files
          <input
            type="file"
            hidden
            multiple
            onChange={handleFilesUpload}
          />
        </Button>
        <Box>
          {files.map((file, index) => (
            <Box
              key={index}
              display="flex"
              justifyContent="space-between"
              alignItems="center"
              sx={{ mt: 1 }}
            >
              <Typography variant="body2">{file.name}</Typography>
              <IconButton color="error" onClick={() => handleFileDelete(index)}>
                <DeleteIcon />
              </IconButton>
            </Box>
          ))}
        </Box>
      </Box>

      {/* Action Buttons */}
      <Box display="flex" justifyContent="space-between">
        <Button
          variant="outlined"
          startIcon={<FolderIcon />}
          sx={{ mr: 2 }}
        >
          New Folder
        </Button>
        <Button variant="contained" color="primary" size="large">
          Create Component
        </Button>
      </Box>
    </Paper>
  );
}
