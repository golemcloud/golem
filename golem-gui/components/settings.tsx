"use client";

import React from "react";
import { Typography, Box, Divider } from "@mui/material";

type SettingProps = {
    title: string;
    description: string;
    actions: {
        title: string;
        description: string;
        buttonText: string;
        onClick: (e:any) => void;
    }[];
    };
const DangerZone = ({ title, description, actions }: SettingProps) => {
  return (
    <div className=" p-4">
      <Box className="flex flex-col">
        <Typography variant="h6">{title}</Typography>
        <Typography variant="subtitle1" gutterBottom>
            {description}
        </Typography>
      </Box>

      <Divider sx={{ borderColor: "#555", marginBottom: "13px" }} />

      {/* Danger Zone Section */}
      <div className="bg-white dark:bg-[#3B191D80] p-6 rounded-lg shadow-lg border border-red-500">
        <h2 className="text-xl font-bold dark:text-gray-300 text-red-950 mb-4">{title}</h2>
        <p className="text-gray-700 dark:text-gray-300 mb-4">{description}</p>

        {actions.map((action, index) => (
          <div key={index} className="flex justify-between items-center mb-4">
            <div>
              <h3 className="text-lg font-semibold text-gray-900 dark:text-gray-100">
                {action.title}
              </h3>
              <p className="text-gray-600 dark:text-gray-400">{action.description}</p>
            </div>
            <button
              className="px-4 py-2 bg-[#3B191D] text-white rounded-lg"
              onClick={action.onClick}
            >
              {action.buttonText}
            </button>
          </div>
        ))}
      </div>
    </div>
  );
};

export default DangerZone;
