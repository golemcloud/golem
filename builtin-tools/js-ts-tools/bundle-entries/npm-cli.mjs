import cli from '../node_modules/npm/lib/cli.js';
import config from '../node_modules/npm/node_modules/@npmcli/config/lib/definitions/index.js';

export const definitions = config.definitions;
export const shorthands = config.shorthands;
export default cli;
