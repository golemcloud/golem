import React, { useState } from 'react';
import { Box, Menu, MenuItem, Chip, TextField } from '@mui/material';

const FilterDropdown: React.FC = () => {
  const [anchorEl, setAnchorEl] = useState<null | HTMLElement>(null);
  const [selectedFilters, setSelectedFilters] = useState<string[]>([]);
  const [currentFilter, setCurrentFilter] = useState<string>('');
  const [menuOptions, setMenuOptions] = useState<string[]>([
    'Running',
    'Idle',
    'Suspended',
    'Interrupted',
    'Retrying',
    'Failed',
    'Exited',
  ]);

  const open = Boolean(anchorEl);

  const handleOpenMenu = (event: React.MouseEvent<HTMLElement>) => {
    setAnchorEl(event.currentTarget);
  };

  const handleCloseMenu = () => {
    setAnchorEl(null);
  };

  const handleSelectFilter = (filter: string) => {
    if (selectedFilters.includes(filter)) {
      setSelectedFilters(selectedFilters.filter((item) => item !== filter)); // Deselect
    } else {
      setSelectedFilters([...selectedFilters, filter]); // Select
    }
  };

  const handleDeleteFilter = (filterToDelete: string) => {
    setSelectedFilters(selectedFilters.filter((filter) => filter !== filterToDelete));
  };

  const handleAddCustomFilter = (event: React.KeyboardEvent<HTMLDivElement>) => {
    if (event.key === 'Enter' && currentFilter.trim() !== '') {
      if (!menuOptions.includes(currentFilter.trim())) {
        setMenuOptions([...menuOptions, currentFilter.trim()]); // Add to options
      }
      if (!selectedFilters.includes(currentFilter.trim())) {
        setSelectedFilters([...selectedFilters, currentFilter.trim()]); // Select
      }
      setCurrentFilter(''); // Clear input
    }
  };

  const renderSelectedFilters = (): string => {
    const displayedFilters = selectedFilters.join(', ');
    return displayedFilters.length > 6 ? `${displayedFilters.slice(0, 6)}...` : displayedFilters;
  };

  // Filter menu options based on search query
  const filteredMenuOptions = menuOptions.filter(option =>
    option.toLowerCase().includes(currentFilter.toLowerCase())
  );

  return (
    <Box
      sx={{
        display: 'flex',
        flexDirection: 'column',
        gap: 2,
        maxWidth: 400,
        padding: 2,
        borderRadius: 0,
        color: 'white',
        width: '200px',
      }}
    >
      <Box
        onClick={handleOpenMenu}
        sx={{
          border: '1px solid gray',
          borderRadius: 1,
          padding: '8px',
          bgcolor: '#333',
          cursor: 'pointer',
          color: 'white',
          height: '40px', // Fixed height for the selection box
          display: 'flex',
          alignItems: 'center',
        }}
      >
        {selectedFilters.length > 0 ? 
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
      </Box> : 'Select Filters...'}
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

        {/* Filtered Dropdown Options */}
        {filteredMenuOptions.map((option) => (
          <MenuItem
            key={option}
            onClick={() => handleSelectFilter(option)}
            sx={{
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
        <MenuItem
          onClick={() => setSelectedFilters(menuOptions)}
          sx={{ justifyContent: 'center' }}
        >
          Select All
        </MenuItem>
      </Menu>      
    </Box>
  );
};

export default FilterDropdown;
