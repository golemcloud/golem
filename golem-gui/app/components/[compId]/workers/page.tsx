"use client"
import React, { useState } from 'react';
import { Box, Button, Typography, TextField, IconButton, Chip } from '@mui/material';
import SearchIcon from '@mui/icons-material/Search';
import RefreshIcon from '@mui/icons-material/Refresh';
import DropdownButton from '@/components/ui/DropDownButton'

const WorkersPage = () => {
  const [searchQuery, setSearchQuery] = useState('');
  const [filters, setFilters] = useState([
    { label: 'Status', value: true },
    { label: 'Version', value: true },
    { label: 'Created After', value: true },
    { label: 'Created Before', value: true },
  ]);

  const handleSearch = (e:any) => {
    setSearchQuery(e.target.value);
  };

  const removeFilter = (label: string) => {
    setFilters((prev) => prev.filter((filter) => filter.label !== label));
  };

  const handleRetry = () => {
    console.log('Retry clicked');
  };

  return (
    <Box
      sx={{
        display: 'flex',
        flexDirection: 'column',
        alignItems: 'center',
        height: '100vh',
        px: 4,
        mt:5
      }}
    >
      <DropdownButton/>
      {/* Search Bar */}
      <Box
        sx={{
          display: 'flex',
          alignItems: 'center',
          width: '100%',
          maxWidth: 800,
          gap: 2,
          mb: 4,
        }}
      >
        <TextField
          variant="outlined"
          placeholder="Worker name..."
          fullWidth
          value={searchQuery}
          onChange={handleSearch}
          InputProps={{
            startAdornment: <SearchIcon sx={{ color: 'gray' }} />,
            sx: { color: 'white' },
          }}
          sx={{ bgcolor: '#222', borderRadius: 1 }}
        />
        <IconButton onClick={() => setSearchQuery('')}>
          <RefreshIcon sx={{ color: 'white' }} />
        </IconButton>
      </Box>

      {/* Filters */}
      <Box
        sx={{
          display: 'flex',
          flexWrap: 'wrap',
          gap: 1,
          mb: 4,
          justifyContent: 'center',
        }}
      >
        {filters.map((filter) => (
          <Chip
            key={filter.label}
            label={filter.label}
            onDelete={() => removeFilter(filter.label)}
            sx={{
              bgcolor: '#333',
              color: 'white',
              '& .MuiChip-deleteIcon': { color: 'gray' },
            }}
          />
        ))}
      </Box>

      {/* No Workers Found Message */}
      <Box
        sx={{
          textAlign: 'center',
          bgcolor: "#1E1E1E",
          p: 4,
          borderRadius: 2,
          width: '100%',
          maxWidth: 600,
        }}
      >
        <Typography variant="h6" sx={{ mb: 2, color: '#888' }}>
          No Workers Found
        </Typography>
        <Typography variant="body1" sx={{ mb: 3 }}>
          No workers matched the current search
        </Typography>
        <Button
          variant="contained"
          sx={{ bgcolor: '#1976d2' }}
          onClick={handleRetry}
        >
          Retry
        </Button>
      </Box>
    </Box>
  );
};

export default WorkersPage;
