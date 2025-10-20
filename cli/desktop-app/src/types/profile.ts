export interface ProfileConfig {
  default_format: string;
}

export interface Profile {
  is_active: boolean;
  name: string;
  url: string;
  config: ProfileConfig;
}
