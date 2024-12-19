"use client";

import React from "react";
import { Typography,Box,Divider } from "@mui/material";
const DangerZone = () => {
  return (
    <div className="min-h-screen  p-4">
      
        <Box className="flex flex-col">
        <Typography variant="h6">
          Api settings
        </Typography>
        <Typography variant="subtitle1" gutterBottom>
          Manage your api settings
        </Typography>
        
      </Box>

      <Divider sx={{ borderColor: "#555",marginBottom:"13px" }} />
      {/* Danger Zone Section */}
      <div className="bg-white dark:bg-[#3B191D80] p-6 rounded-lg shadow-lg border border-red-500">
        <h2 className="text-xl font-bold dark:text-gray-300 text-red-950 mb-4">Danger Zone</h2>
        <p className="text-gray-700 dark:text-gray-300 mb-4">
          Proceed with caution.
        </p>

        {/* Delete Version 0.8 */}
        <div className="flex justify-between items-center mb-4">
          <div>
            <h3 className="text-lg font-semibold text-gray-900 dark:text-gray-100">
              Delete API Version 0.8
            </h3>
            <p className="text-gray-600 dark:text-gray-400">
              Once you delete an API, there is no going back. Please be certain.
            </p>
          </div>
          <button className="px-4 py-2 bg-[#3B191D] text-white rounded-lg">
            Delete Version 0.8
          </button>
        </div>

        {/* Delete All Versions */}
        <div className="flex justify-between items-center">
          <div>
            <h3 className="text-lg font-semibold text-gray-900 dark:text-gray-100">
              Delete all API Versions
            </h3>
            <p className="text-gray-600 dark:text-gray-400">
              Once you delete all API versions, there is no going back. Please
              be certain.
            </p>
          </div>
          <button className="px-4 py-2  bg-[#3B191D] text-white rounded-lg">
            Delete All Versions
          </button>
        </div>
      </div>
    </div>
  );
};

export default DangerZone;
