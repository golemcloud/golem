# Manual Testing Prompts for Golem CLI MCP Server

This document contains a comprehensive list of prompts to test the Golem CLI MCP server integration in Claude Desktop and Gemini CLI.

## Prerequisites

### Transport Modes

The `golem-cli` MCP server supports **two transport modes**:
1. **HTTP mode** (default): For HTTP clients
2. **Stdio mode**: For stdio clients like Claude Desktop and Gemini CLI

### Setup for Claude Desktop (stdio)

1. **Configure Claude Desktop MCP Settings:**
   Add to your Claude Desktop MCP configuration:
   ```json
   {
     "mcpServers": {
       "golem-cli": {
         "command": "golem-cli",
         "args": ["mcp-server", "start", "--transport", "stdio"]
       }
     }
   }
   ```

2. **Verify golem-cli is in PATH:**
   The `golem-cli` executable must be available in your system PATH, or use the full path in the configuration.

---

## Testing Categories

### 1. Basic Tool Discovery Tests

#### Test 1.1: List Available Tools
**Prompt:**
```
What MCP tools are available from the golem-cli server? Please list all available tools and their descriptions.
```

**Expected Result:**
- Should list `list_agent_types` and `list_components`
- Should show tool descriptions
- Should show input schemas

#### Test 1.2: Tool Schema Inspection
**Prompt:**
```
Can you show me the detailed schema for the list_components tool? What parameters does it accept?
```

**Expected Result:**
- Should show the input schema
- Should show parameter requirements
- Should show return type information

---

### 2. Agent Types Testing

#### Test 2.1: List Agent Types (Basic)
**Prompt:**
```
Use the golem-cli MCP server to list all available agent types in Golem.
```

**Expected Result:**
- Should call `list_agent_types` tool
- Should return a list of agent types (or error if Golem not configured)
- Should display results clearly

#### Test 2.2: Agent Types with Error Handling
**Prompt:**
```
Try to get the list of agent types from Golem. If there's an error, explain what it means and whether it's expected.
```

**Expected Result:**
- Should handle errors gracefully
- Should explain if error is due to missing Golem configuration
- Should provide helpful context

#### Test 2.3: Agent Types Analysis
**Prompt:**
```
Get the list of agent types from Golem and tell me:
1. How many agent types are available?
2. What are their names?
3. What can I do with each type?
```

**Expected Result:**
- Should parse the response
- Should extract and count agent types
- Should provide analysis

---

### 3. Components Testing

#### Test 3.1: List Components (Basic)
**Prompt:**
```
Use the golem-cli MCP server to list all available components in my Golem instance.
```

**Expected Result:**
- Should call `list_components` tool
- Should return component list (or error if Golem not configured)
- Should display component information

#### Test 3.2: Components with Details
**Prompt:**
```
Get the list of components from Golem and show me:
- Component IDs
- Component names
- Component sizes
- Any other metadata available
```

**Expected Result:**
- Should parse component data
- Should extract and display all fields
- Should format information clearly

#### Test 3.3: Components Filtering
**Prompt:**
```
List all components from Golem and filter to show only components larger than 1MB (if size information is available).
```

**Expected Result:**
- Should parse component list
- Should filter by size
- Should show filtered results

#### Test 3.4: Components Summary
**Prompt:**
```
Get all components from Golem and provide a summary:
- Total number of components
- Total size of all components
- Average component size
- Largest component
- Smallest component
```

**Expected Result:**
- Should calculate statistics
- Should provide summary analysis
- Should handle empty lists gracefully

---

### 4. Error Handling Tests

#### Test 4.1: Invalid Tool Name
**Prompt:**
```
Try to call a tool named "nonexistent_tool" with empty arguments. What happens?
```

**Expected Result:**
- Should return proper error
- Should show error code and message
- Should explain that tool doesn't exist

#### Test 4.2: Missing Golem Configuration
**Prompt:**
```
If I don't have Golem configured, what happens when I try to list components? What does the error tell me?
```

**Expected Result:**
- Should show error message
- Should explain it's expected if Golem not configured
- Should provide guidance on configuration

#### Test 4.3: Network Error Simulation
**Prompt:**
```
What happens if the MCP server is not running? How would you detect this?
```

**Expected Result:**
- Should explain connection errors
- Should suggest checking server status
- Should provide troubleshooting steps

---

### 5. Integration Workflow Tests

#### Test 5.1: Multi-Step Workflow
**Prompt:**
```
I want to understand my Golem setup. Please:
1. List all available agent types
2. List all available components
3. Give me a summary of what I can do with this setup
```

**Expected Result:**
- Should call both tools
- Should combine results
- Should provide comprehensive summary

#### Test 5.2: Conditional Logic
**Prompt:**
```
Check if I have any components in Golem. If I do, list them. If I don't, tell me how to add components.
```

**Expected Result:**
- Should check component list
- Should provide conditional response
- Should give actionable advice

#### Test 5.3: Data Analysis
**Prompt:**
```
Get all components from Golem and analyze them:
- Group by component type if available
- Identify any patterns
- Suggest which components might be most useful
```

**Expected Result:**
- Should analyze component data
- Should identify patterns
- Should provide recommendations

---

### 6. Tool Chaining Tests

#### Test 6.1: Sequential Tool Calls
**Prompt:**
```
First, get the list of agent types. Then, get the list of components. Finally, suggest which agent types might work best with which components.
```

**Expected Result:**
- Should call tools in sequence
- Should use results from first call
- Should provide intelligent suggestions

#### Test 6.2: Parallel Information Gathering
**Prompt:**
```
Get both the agent types and components in parallel, then create a mapping of which components are compatible with which agent types.
```

**Expected Result:**
- Should call both tools
- Should combine information
- Should create useful mappings

---

### 7. Edge Cases and Validation

#### Test 7.1: Empty Results
**Prompt:**
```
What happens if I have no components in Golem? How does the tool handle an empty list?
```

**Expected Result:**
- Should handle empty lists gracefully
- Should provide helpful message
- Should not crash or error unnecessarily

#### Test 7.2: Large Datasets
**Prompt:**
```
If I have hundreds of components, can the tool handle listing them all? Test with the current setup.
```

**Expected Result:**
- Should handle large lists
- Should paginate or summarize if needed
- Should remain responsive

#### Test 7.3: Special Characters
**Prompt:**
```
Do component names or agent type names support special characters? Test with the current data.
```

**Expected Result:**
- Should handle special characters
- Should display correctly
- Should not break parsing

---

### 8. User Experience Tests

#### Test 8.1: Clear Explanations
**Prompt:**
```
I'm new to Golem. Can you explain what agent types and components are, and then show me what I have available?
```

**Expected Result:**
- Should provide educational context
- Should explain concepts clearly
- Should then show actual data

#### Test 8.2: Actionable Recommendations
**Prompt:**
```
Based on my available components and agent types, what should I do next? Give me specific recommendations.
```

**Expected Result:**
- Should analyze available resources
- Should provide specific next steps
- Should be actionable

#### Test 8.3: Help and Documentation
**Prompt:**
```
I don't know how to use the Golem MCP tools. Can you show me examples of what each tool does?
```

**Expected Result:**
- Should explain each tool
- Should provide usage examples
- Should show sample outputs

---

### 9. Performance Tests

#### Test 9.1: Response Time
**Prompt:**
```
How fast can you get the list of components? Time the operation and tell me how long it took.
```

**Expected Result:**
- Should measure response time
- Should report timing
- Should be reasonably fast (< 5 seconds)

#### Test 9.2: Multiple Rapid Calls
**Prompt:**
```
Call list_components 5 times in a row and see if there are any performance issues or errors.
```

**Expected Result:**
- Should handle multiple calls
- Should maintain performance
- Should not cause errors

---

### 10. Advanced Integration Tests

#### Test 10.1: Combining with Other Tools
**Prompt:**
```
Use the Golem MCP tools to get my components, then use file system tools to check if any component files exist locally, and compare the results.
```

**Expected Result:**
- Should use MCP tools
- Should combine with other capabilities
- Should provide cross-referenced results

#### Test 10.2: Code Generation Based on MCP Data
**Prompt:**
```
Get my list of components from Golem, then generate Python code that could interact with those components.
```

**Expected Result:**
- Should fetch component data
- Should generate relevant code
- Should use actual component information

#### Test 10.3: Documentation Generation
**Prompt:**
```
Get all my Golem components and agent types, then create a markdown document summarizing my Golem setup.
```

**Expected Result:**
- Should fetch all data
- Should create formatted document
- Should be comprehensive

---

## Testing Checklist

Use this checklist to verify all functionality:

### Basic Functionality
- [ ] Can discover available tools
- [ ] Can call `list_agent_types` successfully
- [ ] Can call `list_components` successfully
- [ ] Tools return proper JSON responses
- [ ] Error handling works correctly

### Error Handling
- [ ] Invalid tool names return proper errors
- [ ] Missing Golem config shows helpful errors
- [ ] Network errors are handled gracefully
- [ ] Empty results are handled properly

### Integration
- [ ] Multiple tools can be called in sequence
- [ ] Results can be combined and analyzed
- [ ] Tools work with other MCP client capabilities
- [ ] Performance is acceptable

### User Experience
- [ ] Responses are clear and helpful
- [ ] Errors provide actionable guidance
- [ ] Complex queries are handled well
- [ ] Documentation is accessible

---

## Expected Tool Responses

### `list_agent_types` Success Response:
```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "content": [
      {
        "type": "text",
        "text": "{\"agent_types\":[\"type1\",\"type2\",...]}"
      }
    ]
  }
}
```

### `list_components` Success Response:
```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "result": {
    "content": [
      {
        "type": "text",
        "text": "{\"components\":[{\"id\":\"...\",\"name\":\"...\",\"revision\":0,\"size\":1024},...]}"
      }
    ]
  }
}
```

### Error Response (Tool Not Found):
```json
{
  "jsonrpc": "2.0",
  "id": 3,
  "error": {
    "code": -32601,
    "message": "Method not found: nonexistent_tool"
  }
}
```

### Error Response (Golem Not Configured):
```json
{
  "jsonrpc": "2.0",
  "id": 4,
  "error": {
    "code": -32000,
    "message": "Golem environment not configured: ..."
  }
}
```

---

## Troubleshooting

### If tools are not available:
1. Check server is running: `python list_mcp_tools.py`
2. Verify MCP settings are correct
3. Restart the MCP client to reload configuration
4. Check server logs for errors

### If tools return errors:
1. Verify Golem is configured (if needed for the tool)
2. Check network connectivity
3. Verify server is accessible at `http://127.0.0.1:3000/mcp`
4. Check server logs for detailed error messages

### If responses are slow:
1. Check server performance
2. Verify network latency
3. Check if Golem services are responding
4. Review server logs for bottlenecks

---

## Success Criteria

The MCP integration is working correctly if:
- ✅ All tools are discoverable
- ✅ Tools can be called successfully
- ✅ Responses are properly formatted
- ✅ Errors are handled gracefully
- ✅ Multiple tools can be used in workflows
- ✅ Performance is acceptable (< 5s per call)
- ✅ Integration with other MCP client features works

---

## Notes

- Some tools may return errors if Golem is not fully configured - this is expected
- The MCP server uses connection-based sessions, so each request should maintain the connection
- Tools return JSON data that needs to be parsed
- Error messages should be clear and actionable
