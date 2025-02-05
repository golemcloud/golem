import {Api} from "@/types/api";
import {ComponentExportFunction, Export, FileStructure} from "@/types/component";
import {type ClassValue, clsx} from "clsx";
import {twMerge} from "tailwind-merge";

export function cn(...inputs: ClassValue[]) {
    return twMerge(clsx(inputs));
}

export function formatRelativeTime(dateString: string | number | Date) {
    const date = new Date(dateString).getTime();
    const now = new Date().getTime();
    const diffInSeconds = Math.floor((now - date) / 1000);

    const units = [
        {name: "year", seconds: 60 * 60 * 24 * 365},
        {name: "month", seconds: 60 * 60 * 24 * 30},
        {name: "week", seconds: 60 * 60 * 24 * 7},
        {name: "day", seconds: 60 * 60 * 24},
        {name: "hour", seconds: 60 * 60},
        {name: "minute", seconds: 60},
        {name: "second", seconds: 1},
    ];

    for (const unit of units) {
        if (diffInSeconds >= unit.seconds) {
            const value = Math.floor(diffInSeconds / unit.seconds);
            return `${value} ${unit.name}${value > 1 ? "s" : ""} ago`;
        }
    }

    return "just now";
}

export const sanitizeInput = (input: string): string => {
    return input.replace(/\u201c|\u201d/g, '"').replace(/'/g, '"');
};

export function formatTimestampInDateTimeFormat(timestamp: string) {
    const date = new Date(timestamp);

    // Get date components
    const month = String(date.getMonth() + 1).padStart(2, "0"); // Months are zero-indexed
    const day = String(date.getDate()).padStart(2, "0");

    // Get time components
    const hours = String(date.getHours()).padStart(2, "0");
    const minutes = String(date.getMinutes()).padStart(2, "0");
    const seconds = String(date.getSeconds()).padStart(2, "0");
    const milliseconds = String(date.getMilliseconds()).padStart(3, "0");

    // Combine into the desired format
    return `${month}-${day} ${hours}:${minutes}:${seconds}.${milliseconds}`;
}

/// compare semver version
export const compareSemver = (version1: string, version2: string) => {
    const semverRegex =
        /^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?(?:\+([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?$/;

    if (!semverRegex.test(version1) || !semverRegex.test(version2)) {
        throw new Error("Invalid semver version format");
    }

    const v1Parts = version1.split(".");
    const v2Parts = version2.split(".");

    // Compare major version
    const major1 = parseInt(v1Parts[0]);
    const major2 = parseInt(v2Parts[0]);
    if (major1 !== major2) return major1 > major2;

    const minor1 = parseInt(v1Parts[1]);
    const minor2 = parseInt(v2Parts[1]);
    if (minor1 !== minor2) return minor1 > minor2;

    // Compare patch version
    const patch1 = parseInt(v1Parts[2]);
    const patch2 = parseInt(v2Parts[2]);
    return patch1 > patch2;

    return false;
};

/// Remove the duplicate api and keep the latest one by comparing the semver version
export const removeDuplicateApis = (data: Api[]) => {
    const uniqueEntries = {} as Record<string, Api>;

    data.forEach((item) => {
        if (!uniqueEntries[item.id]) {
            uniqueEntries[item.id] = item;
        } else {
            // check semver for latest version
            const uniqueEntriesVersion = uniqueEntries[item.id].version;
            const count = (uniqueEntries[item.id].count || 1) + 1;
            const itemVersion = item.version;
            if (compareSemver(itemVersion, uniqueEntriesVersion)) {
                uniqueEntries[item.id] = {...item, count: count + 1};
            } else {
                uniqueEntries[item.id].count = count;
            }
        }
    });
    return Object.values(uniqueEntries);
};

export const parseErrorMessage = (error: string): string => {
    const patterns = [
        /(?<=: ).*?(?=\s\(occurred)/, // Extract message before "(occurred"
        /(?<=error: ).*?(?=:|$)/i, // Extract message after "error: "
        /Invalid value for the key [^:]+/, // Extract key-specific error
    ];

    for (const pattern of patterns) {
        const match = error.match(pattern);
        if (match) {
            return match[0];
        }
    }
    return "An unknown error occurred.";
};

export const calculateExportFunctions = (exports: Export[]) => {
    const functions = exports.reduce(
        (acc: ComponentExportFunction[], curr: Export) => {
            const updatedFunctions = curr.functions.map(
                (func: ComponentExportFunction) => ({
                    ...func,
                    exportName: curr.name,
                })
            );

            return acc.concat(updatedFunctions);
        },
        []
    );
    return functions;
};


interface FileNode {
    name: string
    type: "file" | "folder"
    children?: FileNode[]
    permissions?: string
}

export function parseFileStructure(data: FileStructure[]): FileNode {
    const root: FileNode = {name: "root", type: "folder", children: []}

    data.forEach((item) => {
        const parts = item.path.split("/").filter(Boolean)
        let currentNode = root

        parts.forEach((part: string, index: number) => {
            if (index === parts.length - 1) {
                currentNode.children?.push({
                    name: part,
                    type: "file",
                    permissions: item.permissions,
                })
            } else {
                let folderNode = currentNode.children?.find((child) => child.name === part && child.type === "folder")
                if (!folderNode) {
                    folderNode = {name: part, type: "folder", children: []}
                    currentNode.children?.push(folderNode)
                }
                currentNode = folderNode
            }
        })
    })

    return root
}
