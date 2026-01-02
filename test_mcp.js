// Quick MCP client test
const http = require('http');

const data = JSON.stringify({
  jsonrpc: '2.0',
  id: 1,
  method: 'tools/list',
  params: {}
});

const options = {
  hostname: '127.0.0.1',
  port: 3000,
  path: '/mcp',
  method: 'POST',
  headers: {
    'Content-Type': 'application/json',
    'Content-Length': data.length
  }
};

console.log('Testing MCP endpoint...\n');

const req = http.request(options, (res) => {
  console.log(`Status: ${res.statusCode}`);
  console.log(`Headers: ${JSON.stringify(res.headers)}\n`);

  let body = '';
  res.on('data', (chunk) => {
    body += chunk;
  });

  res.on('end', () => {
    console.log('Response:');
    try {
      const json = JSON.parse(body);
      console.log(JSON.stringify(json, null, 2));
      
      if (json.result && json.result.tools) {
        console.log(`\n✓ SUCCESS: Found ${json.result.tools.length} tools`);
        json.result.tools.forEach(tool => {
          console.log(`  - ${tool.name}: ${tool.description}`);
        });
      }
    } catch (e) {
      console.log(body);
    }
  });
});

req.on('error', (e) => {
  console.error(`✗ ERROR: ${e.message}`);
});

req.write(data);
req.end();
