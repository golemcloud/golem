import { Typ } from "./component";

export interface AgentTypeSchema {
  agentType: {
    typeName: string;
    description: string;
    constructor: {
      name: string;
      description: string;
      promptHint: string;
      inputSchema: {
        type: "Tuple";
        elements: Array<{
          name: string;
          schema: {
            type: "ComponentModel";
            elementType: Typ;
          };
        }>;
      };
    };
    methods: Array<{
      name: string;
      description: string;
      promptHint: string;
      inputSchema: {
        type: "Tuple";
        elements: Array<{
          name: string;
          schema: {
            type: "ComponentModel";
            elementType: Typ;
          };
        }>;
      };
      outputSchema: {
        type: "Tuple";
        elements: Array<{
          name: string;
          schema: {
            type: "ComponentModel";
            elementType: Typ;
          };
        }>;
      };
    }>;
    dependencies: string[];
  };
  implementedBy: {
    componentId: string;
    componentRevision: number;
  };
}
