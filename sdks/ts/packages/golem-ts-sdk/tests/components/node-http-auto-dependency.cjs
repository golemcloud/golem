const { createServer } = require('http');
module.exports = createServer((_req, res) => res.end('dependency')).listen(3001);
