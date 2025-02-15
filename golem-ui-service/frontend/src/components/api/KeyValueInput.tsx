import { Plus, X } from "lucide-react";
import React, { useState } from "react";

interface KeyValueInputProps {
  label: string;
  value: Record<string, string>;
  onChange: (value: Record<string, string>) => void;
  editableKeys?: boolean;
}

export const KeyValueInput = ({
  label,
  value,
  onChange,
  editableKeys = true,
}: KeyValueInputProps) => {
  const [newKey, setNewKey] = useState("");
  const [newValue, setNewValue] = useState("");

  const handleAdd = () => {
    if (newKey && newValue) {
      onChange({ ...value, [newKey]: newValue });
      setNewKey("");
      setNewValue("");
    }
  };

  const handleKeyChange = (oldKey: string, newKey: string) => {
    const newPairs = { ...value };
    const val = newPairs[oldKey];
    delete newPairs[oldKey];
    newPairs[newKey] = val;
    onChange(newPairs);
  };

  const handleValueChange = (key: string, newVal: string) => {
    onChange({ ...value, [key]: newVal });
  };

  const handleRemove = (key: string) => {
    const newValue = { ...value };
    delete newValue[key];
    onChange(newValue);
  };

  return (
    <div className="space-y-4">
      <label className="block text-sm font-medium">{label}</label>

      {/* Existing key-value pairs */}
      <div className="space-y-2">
        {Object.entries(value).map(([key, val]) => (
          <div key={key} className="flex items-center gap-2">
            <input
              type="text"
              value={key}
              disabled={!editableKeys}
              onChange={(e) => handleKeyChange(key, e.target.value)}
              className="bg-gray-800 w-full p-2 rounded-md border border-gray-700 focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
            />
            <input
              type="text"
              value={val}
              onChange={(e) => handleValueChange(key, e.target.value)}
              className="bg-gray-800 w-full p-2 rounded-md border border-gray-700 focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
            />
            <button
              onClick={() => handleRemove(key)}
              className="p-2 text-gray-400 hover:text-red-400 hover:bg-gray-700 rounded-md transition-colors"
              title="Remove"
            >
              <X size={16} />
            </button>
          </div>
        ))}
      </div>

      {/* Add new key-value pair */}
      <div className="flex items-center gap-2">
        <input
          type="text"
          value={newKey}
          onChange={(e) => setNewKey(e.target.value)}
          placeholder="Key"
          className="bg-gray-800 w-full p-2 rounded-md border border-gray-700 focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
        />
        <input
          type="text"
          value={newValue}
          onChange={(e) => setNewValue(e.target.value)}
          placeholder="Value"
          className="bg-gray-800 w-full p-2 rounded-md border border-gray-700 focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
        />
        <button
          onClick={handleAdd}
          disabled={!newKey || !newValue}
          className="p-2 bg-blue-500 text-white rounded-md hover:bg-blue-600 disabled:opacity-50 disabled:hover:bg-blue-500"
          title="Add"
        >
          <Plus size={16} />
        </button>
      </div>
    </div>
  );
};

export default KeyValueInput;
