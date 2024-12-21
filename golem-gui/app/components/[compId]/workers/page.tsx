"use client"

import React, { useState } from 'react';
import {
  Box,
  TextField,
  Button,
  Typography,
  Stack,
  IconButton,
  Select,
  MenuItem,
  FormControl,
  InputLabel,
  ListSubheader,
} from '@mui/material';
import RefreshIcon from '@mui/icons-material/Refresh';
import AddIcon from '@mui/icons-material/Add';
import { LocalizationProvider } from '@mui/x-date-pickers/LocalizationProvider';
import { AdapterDateFns } from '@mui/x-date-pickers/AdapterDateFnsV3';
import { DatePicker } from '@mui/x-date-pickers/DatePicker';

const WorkerListWithDropdowns = () => {
  const [workerStatus, setWorkerStatus] = useState('');
  const [version, setVersion] = useState('');
  const [createdAfter, setCreatedAfter] = useState<Date | null>(null);
  const [createdBefore, setCreatedBefore] = useState<Date | null>(null);
  const [searchQuery, setSearchQuery] = useState(''); // For searching statuses

  const statuses = ['Running', 'Idle', 'Suspended', 'Interrupted', 'Retrying', 'Failed', 'Exited'];

  const filteredStatuses = statuses.filter((status) =>
    status.toLowerCase().includes(searchQuery.toLowerCase())
  );

  return (
    <LocalizationProvider dateAdapter={AdapterDateFns}>
      <Box
        sx={{
          marginBottom: 3,
          padding: 3,
          display: 'flex',
          flexDirection: 'column',
        }}
      >
        {/* Search Box */}
        <Stack direction="row" spacing={2} mb={3}>
          <TextField
            placeholder="Worker name..."
            variant="outlined"
            fullWidth
            InputProps={{
              startAdornment: (
                <Typography sx={{ marginRight: 1 }}>🔍</Typography>
              ),
            }}
          />
          <IconButton sx={{ color: 'white' }}>
            <RefreshIcon />
          </IconButton>
          <Button
            variant="contained"
            startIcon={<AddIcon />}
            sx={{
              backgroundColor: '#2962FF',
              '&:hover': { backgroundColor: '#0039CB' },
            }}
          >
            New
          </Button>
        </Stack>

        {/* Dropdowns and Date Pickers */}
        <Stack direction="row" spacing={2} mb={3}>
          {/* Worker Status with Search */}
          <FormControl variant="outlined" size="medium" sx={{ minWidth: 150 }}>
            <Select
              value={workerStatus}
              onChange={(e) => setWorkerStatus(e.target.value)}
              MenuProps={{
                PaperProps: {
                  sx: {
                    maxHeight: 300,
                  },
                },
              }}
              displayEmpty
            >
              <ListSubheader>
                <TextField
                  placeholder="Search..."
                  variant="standard"
                  fullWidth
                  InputProps={{
                    disableUnderline: true,
                    startAdornment: (
                      <Typography sx={{ marginRight: 1 }}>🔍</Typography>
                    ),
                  }}
                  value={searchQuery}
                  onChange={(e) => setSearchQuery(e.target.value)}
                  sx={{ padding: 1,
                    borderRadius: 1,
                    border: '1px solid gray',
                   }}
                />
              </ListSubheader>
              {filteredStatuses.map((status) => (
                <MenuItem key={status} value={status}>
                  {status}
                </MenuItem>
              ))}
              {filteredStatuses.length === 0 && (
                <MenuItem disabled>No results found</MenuItem>
              )}
            </Select>
          </FormControl>

          {/* Version */}
          <FormControl variant="outlined" size="medium" sx={{ minWidth: 150 }}>
  <Select
    value={version}
    onChange={(e) => setVersion(e.target.value)}
    MenuProps={{
      PaperProps: {
        sx: {
          maxHeight: 300, // Control dropdown height
        },
      },
    }}
    displayEmpty
  >
    <ListSubheader>
      <TextField
        placeholder="Search..."
        variant="standard"
        fullWidth
        InputProps={{
          disableUnderline: true,
          startAdornment: (
            <Typography sx={{ marginRight: 1 }}>🔍</Typography>
          ),
        }}
        value={searchQuery}
        onChange={(e) => setSearchQuery(e.target.value)}
        sx={{ 
          padding: 1,
          borderRadius: 1,
          border: '1px solid gray',
         }}
      />
    </ListSubheader>
    {['v1', 'v2', 'v3'].filter((v) =>
      v.toLowerCase().includes(searchQuery.toLowerCase())
    ).map((v) => (
      <MenuItem key={v} value={v}>
        {v}
      </MenuItem>
    ))}
    {['v1', 'v2', 'v3'].filter((v) =>
      v.toLowerCase().includes(searchQuery.toLowerCase())
    ).length === 0 && <MenuItem disabled>No results found</MenuItem>}
  </Select>
</FormControl>


          {/* Created After */}
          <DatePicker
            label="Created After"
            value={createdAfter}
            onChange={(date) => setCreatedAfter(date)}
            renderInput={(params) => (
              <TextField
                {...params}
                sx={{
                  '.MuiOutlinedInput-notchedOutline': { borderColor: 'gray' },
                }}
              />
            )}
          />

          {/* Created Before */}
          <DatePicker
            label="Created Before"
            value={createdBefore}
            onChange={(date) => setCreatedBefore(date)}
            renderInput={(params) => (
              <TextField
                {...params}
                sx={{
                  '.MuiOutlinedInput-notchedOutline': { borderColor: 'gray' },
                }}
              />
            )}
          />
        </Stack>

        {/* No Workers Found */}
        <Box
          className="dark:bg-gray-800 bg-[#E3F2FD] dark:text-white text-black"
          sx={{
            flex: 1,
            display: 'flex',
            justifyContent: 'center',
            alignItems: 'center',
            flexDirection: 'column',
            padding: 3,
            borderRadius: 1,
          }}
        >
          <Typography variant="h6" sx={{ mb: 1 }}>
            No Workers Found
          </Typography>
          <Typography variant="body2" sx={{ mb: 2 }}>
            No workers matched the current search
          </Typography>
          <Button
            variant="contained"
            sx={{
              '&:hover': { backgroundColor: '#0039CB' },
            }}
          >
            Retry
          </Button>
        </Box>
      </Box>
    </LocalizationProvider>
  );
};

export default WorkerListWithDropdowns;
