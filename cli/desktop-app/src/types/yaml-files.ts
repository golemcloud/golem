export interface YamlFile {
  /** The display name for the tab */
  name: string;
  /** The relative path of the file */
  path: string;
  /** The YAML content */
  content: string;
  /** The type of YAML file */
  type: "root" | "common" | "component";
  /** Whether this file can be edited */
  editable: boolean;
}

export interface AppYamlFiles {
  /** The root golem.yaml file */
  root: YamlFile;
  /** Common YAML files from common-* directories */
  common: YamlFile[];
  /** Component YAML files from components-*/
  components: YamlFile[];
}
