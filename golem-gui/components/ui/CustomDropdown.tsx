"use client"

import React, { useState } from 'react';
import { Box, Menu, MenuItem, Chip, TextField, Typography } from '@mui/material';


export default function CustomDropdown() {
  const [anchorEl, setAnchorEl] = useState(null);
  const [selectedFilters, setSelectedFilters] = useState([]);
  const [currentFilter, setCurrentFilter] = useState('');
  const [menuOptions, setMenuOptions] = useState([
    'Running',
    'Idle',
    'Suspended',
    'Interrupted',
    'Retrying',
    'Failed',
    'Exited',
  ]);

  const open = Boolean(anchorEl);

  const handleOpenMenu = (event:any) => {
    setAnchorEl(event.currentTarget);
  };

  const handleCloseMenu = () => {
    setAnchorEl(null);
  };

  const handleSelectFilter = (filter:string) => {
        // @ts-ignore
    if (!selectedFilters.includes(filter)) {
        // @ts-ignore
      setSelectedFilters([...selectedFilters, filter]);
    }
  };

  // @ts-ignore
  const handleDeleteFilter = (filterToDelete) => {
    setSelectedFilters(selectedFilters.filter((filter) => filter !== filterToDelete));
  };

  const handleAddCustomFilter = (event:any) => {
    if (event.key === 'Enter' && currentFilter.trim() !== '') { // @ts-ignore
      if (!selectedFilters.includes(currentFilter.trim())) { // @ts-ignore
        setSelectedFilters([...selectedFilters, currentFilter.trim()]);
        setMenuOptions([...menuOptions, currentFilter.trim()]); // Add to menu options
      }
      setCurrentFilter(''); // Clear input
    }
  };

  return (
    <Box
      sx={{
        display: 'flex',
        flexDirection: 'column',
        gap: 2,
        maxWidth: 400,
        bgcolor: 'black',
        padding: 2,
        borderRadius: 1,
        color: 'white',
      }}
    >
      {/* Trigger for Dropdown */}
      <Box
        onClick={handleOpenMenu}
        sx={{
          border: '1px solid gray',
          borderRadius: 1,
          padding: '8px',
          bgcolor: '#333',
          cursor: 'pointer',
          color: 'white',
        }}
      >
        {selectedFilters.length > 0
          ? selectedFilters.join(', ')
          : 'Select Filters...'}
      </Box>

      {/* Dropdown Menu */}
      <Menu
        anchorEl={anchorEl}
        open={open}
        onClose={handleCloseMenu}
        sx={{
          '& .MuiPaper-root': {
            bgcolor: '#222',
            color: 'white',
          },
        }}
      >
        {/* Searchable Input */}
        <MenuItem disableRipple>
          <TextField
            value={currentFilter}
            onChange={(e) => setCurrentFilter(e.target.value)}
            onKeyDown={handleAddCustomFilter}
            placeholder="Type to search or add"
            fullWidth
            variant="standard"
            InputProps={{
              style: {
                color: 'white',
              },
            }}
            sx={{
              '& .MuiInput-underline:before': {
                borderBottom: '1px solid gray',
              },
              '& .MuiInput-underline:hover:not(.Mui-disabled):before': {
                borderBottom: '1px solid white',
              },
            }}
          />
        </MenuItem>

        {/* Dropdown Options */}
        {menuOptions.map((option) => (
          <MenuItem
            key={option}
            onClick={() => handleSelectFilter(option)}
            sx={{// @ts-ignore
              bgcolor: selectedFilters.includes(option) ? '#444' : 'transparent',
              '&:hover': { bgcolor: '#555' },
            }}
          >
            {option}
          </MenuItem>
        ))}

        {/* Clear and Select All Options */}
        <MenuItem
          onClick={() => setSelectedFilters([])}
          sx={{ borderTop: '1px solid gray', justifyContent: 'center' }}
        >
          Clear
        </MenuItem>
        <MenuItem // @ts-ignore
          onClick={() => setSelectedFilters(menuOptions)}
          sx={{ justifyContent: 'center' }}
        >
          Select All
        </MenuItem>
      </Menu>

      {/* Display Selected Filters as Chips */}
      <Box sx={{ display: 'flex', gap: 1, flexWrap: 'wrap' }}>
        {selectedFilters.map((filter) => (
          <Chip
            key={filter}
            label={filter}
            onDelete={() => handleDeleteFilter(filter)}
            sx={{
              bgcolor: '#333',
              color: 'white',
              '& .MuiChip-deleteIcon': { color: 'gray' },
            }}
          />
        ))}
      </Box>
    </Box>
  );
};

