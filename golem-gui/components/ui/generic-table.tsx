import React from "react";
import Table from "@mui/material/Table";
import TableBody from "@mui/material/TableBody";
import TableCell from "@mui/material/TableCell";
import TableContainer from "@mui/material/TableContainer";
import TableHead from "@mui/material/TableHead";
import TableRow from "@mui/material/TableRow";
import Paper from "@mui/material/Paper";

interface Column<T> {
  key: string;
  label: string;
  accessor: (item: T) => React.ReactNode;
  align?: "left" | "center" | "right";
}

interface GenericTableProps<T> {
  data: T[];
  columns: Column<T>[];
  onRowClick?: (item: T) => void;
}

const GenericTable = <T,>({
  data,
  columns,
  onRowClick,
}: GenericTableProps<T>) => {
  return (
    <TableContainer
      component={Paper}
      className="bg-white dark:bg-[#333] rounded-sm shadow-md border border-gray-400"
    >
      <Table className="min-w-full" aria-label="generic table">
        <TableHead className="dark:bg-[#1a2241] bg-[#f0f4ff]">
          <TableRow>
            {columns.map((column, index) => (
              <TableCell
                key={index}
                className="text-gray-900 dark:text-gray-100"
                align={column.align || "left"}
              >
                {column.label}
              </TableCell>
            ))}
          </TableRow>
        </TableHead>
        <TableBody>
          {data.map((item, rowIndex) => (
            <TableRow
              key={rowIndex}
              onClick={() => onRowClick && onRowClick(item)}
              className="border-b dark:border-gray-700 hover:bg-gray-100 dark:hover:bg-gray-700 cursor-pointer"
            >
              {columns.map((column, colIndex) => (
                <TableCell
                  key={colIndex}
                  align={column.align || "left"}
                  className="text-gray-900 dark:text-gray-100"
                >
                  {/* Handle the case where accessor returns an array of objects */}
                  {Array.isArray(column.accessor(item)) ? (
                    column.accessor(item).map((subItem, index) => (
                      <div key={index}>
                        {JSON.stringify(subItem)}:
                         `( ${subItem.name} )`
                      </div>
                    ))
                  ) : (
                    column.accessor(item) ?? "-" // Fallback if undefined
                  )}
                </TableCell>
              ))}
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </TableContainer>
  );
};


export default GenericTable;
