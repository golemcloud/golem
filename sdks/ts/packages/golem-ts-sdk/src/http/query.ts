import { QueryVariable } from 'golem:agent/common';
import { validateIdentifier } from './identifier';

export function parseQuery(query: string): QueryVariable[] {
  if (!query) return [];

  return query.split('&').map((pair) => {
    const [key, value] = pair.split('=');

    if (!key || !value) {
      throw new Error(`Invalid query segment "${pair}"`);
    }

    if (!value.startsWith('{') || !value.endsWith('}')) {
      throw new Error(`Query value for "${key}" must be a variable reference`);
    }

    const variableName = value.slice(1, -1);
    validateIdentifier(variableName);

    return {
      queryParamName: key,
      variableName,
    };
  });
}
