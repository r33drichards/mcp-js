#!/bin/bash

# Test script for mcp-v8 MCP project with stdio transport

# Exit on error
set -e

# Colors for output
GREEN='\033[0;32m'
BLUE='\033[0;34m'
RED='\033[0;31m'
NC='\033[0m' # No Color

echo -e "${BLUE}Building server...${NC}"
cd server
cargo build

echo -e "${BLUE}Building client...${NC}"
cd ../client
cargo build

echo -e "${BLUE}Testing Method 1: Direct JSON-RPC communication${NC}"
cd ..
echo -e "${GREEN}Creating a test input file...${NC}"
cat > test_input.json << EOF
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocol_version":"2024-11-05"}}
{"jsonrpc":"2.0","id":2,"method":"tool_call","params":{"name":"javascript","parameters":{"code":"x=1;x+=1;x"}}}
{"jsonrpc":"2.0","id":3,"method":"shutdown","params":{}}
EOF

echo -e "${GREEN}Running server with test input...${NC}"
./server/target/debug/mcp-v8-server < test_input.json > test_output.json

echo -e "${GREEN}Checking server output...${NC}"
if grep -q "Hello, MCP User" test_output.json; then
    echo -e "${GREEN}Direct JSON-RPC test completed successfully!${NC}"
else
    echo -e "${RED}Direct JSON-RPC test failed. Server output does not contain expected response.${NC}"
    cat test_output.json
    exit 1
fi

# Clean up
rm test_input.json test_output.json

echo -e "${BLUE}Testing Method 2: Client starting server${NC}"
echo -e "${GREEN}Running client in one-shot mode...${NC}"
./client/target/debug/mcp-v8-client --name "MCP Tester" > client_output.txt

echo -e "${GREEN}Checking client output...${NC}"
if grep -q "Hello, MCP Tester" client_output.txt; then
    echo -e "${GREEN}Client-server test completed successfully!${NC}"
else
    echo -e "${RED}Client-server test failed. Client output does not contain expected response.${NC}"
    cat client_output.txt
    exit 1
fi

# Clean up
rm client_output.txt

echo -e "${BLUE}Testing Method 3: Client connecting to running server${NC}"
echo -e "${GREEN}Starting server in background...${NC}"
./server/target/debug/mcp-v8-server &
SERVER_PID=$!

# Give the server a moment to start
sleep 1

echo -e "${GREEN}Running client in connect mode...${NC}"
./client/target/debug/mcp-v8-client --connect --name "Connected User" > connect_output.txt

echo -e "${GREEN}Checking client output...${NC}"
if grep -q "Hello, Connected User" connect_output.txt; then
    echo -e "${GREEN}Connect mode test completed successfully!${NC}"
else
    echo -e "${RED}Connect mode test failed. Client output does not contain expected response.${NC}"
    cat connect_output.txt
    kill $SERVER_PID
    exit 1
fi

# Clean up
rm connect_output.txt
kill $SERVER_PID

echo -e "${GREEN}All tests completed successfully!${NC}"
