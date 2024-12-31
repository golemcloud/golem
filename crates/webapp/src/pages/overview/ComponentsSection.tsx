import {Layers, PlusCircle} from 'lucide-react';
import { useNavigate } from 'react-router-dom';
import {Button} from "@/components/ui/button.tsx";
import {useEffect, useState} from "react";

import { invoke } from '@tauri-apps/api/core';


const ComponentsSection = () => {
  const navigate = useNavigate();
  const [components, setComponents] = useState([]);
  useEffect(() => {
    const fetchData = async () => {
      const response: any = await invoke('get_component');
      setComponents(response);
      console.log(response);
    };
    fetchData().then(r => r);
  }, []);
    console.log(components, "components");
  return (
    <div className="bg-white rounded-lg border border-gray-200 p-6">
        <div className="flex justify-between items-center mb-6">
            {JSON.stringify(components)}
        </div>
      <div className="flex justify-between items-center mb-6">
        <h2 className="text-xl font-semibold">Components</h2>
        <button className="text-blue-600 hover:text-blue-700" onClick={() =>{
          navigate("/components");
        }}>View All</button>
      </div>
      <div className="flex flex-col items-center justify-center py-12 border-2 border-dashed border-gray-200 rounded-lg">
        <Layers className="h-12 w-12 text-gray-400 mb-4" />
        <h3 className="text-lg font-medium mb-2">No Components</h3>
        <p className="text-gray-500 mb-4">Create your first component to get started</p>
      <Button  onClick={() =>{navigate("/components/create");}}>
          <PlusCircle className="mr-2 size-4" />
          Create Component
      </Button>
      </div>
    </div>
  );
};

export default ComponentsSection;