export enum Status {
  Success = "Success",
  Error = "Error",
}

export interface Response<T> {
  status: Status;
  data?: T;
  error?: string;
}

export interface ProfileConfig {
  default_format: string;
}

export interface Profile {
  is_active: boolean;
  name: string;
  kind: "Cloud" | "Oss";
  url?: string;
  config: ProfileConfig;
}
