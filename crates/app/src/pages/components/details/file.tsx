import {FolderStructure} from "@/components/file-manager.tsx";
import {useParams} from "react-router-dom";
import {useEffect, useState} from "react";
import {API} from "@/service";
import {Component} from "@/types/component.ts";

export default function FileManager() {
    const {componentId = ""} = useParams();
    const [component, setComponent] = useState({} as Component);

    useEffect(() => {
        if (!componentId) return;
        // Fetch entire list of components by ID
        API.getComponentById(componentId).then((response) => {
            if (!response) return;
            setComponent(response[0]);
        });
    }, [componentId]);
    return (
        <div className="container mx-auto p-4">
            <FolderStructure data={component.files || []}/>
        </div>
    )
}

