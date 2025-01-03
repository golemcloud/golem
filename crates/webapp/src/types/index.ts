export enum Status {
    Success = "Success",
    Error = "Error"
}

export interface Response<T> {
    status: Status;
    data?: T;
    error?: string;
}

export interface Plugin {
    name: string
    version: string
    description: string
    homepage: string
    specs: {
        type: string
        componentId?: string
        componentVersion?: number
        jsonSchema?: string
        validateUrl?: string
        transformUrl?: string
    }
    scope: {
        type: string
        componentID?: string
    }

}