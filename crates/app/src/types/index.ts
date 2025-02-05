export enum Status {
    Success = "Success",
    Error = "Error"
}

export interface Response<T> {
    status: Status;
    data?: T;
    error?: string;
}

