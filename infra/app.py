#!/usr/bin/env python3
import os
import aws_cdk as cdk
from agent_b_mcp_server_stack import AgentBMCPSStack
from agent_a_mcp_server_stack import AgentAMCPServerStack
from agent_a_mcp_client_stack import AgentAMCPClientStack

env = cdk.Environment(
    account="940333627479",
    region="us-east-1",
)

app = cdk.App()

# Deploy Agent B MCP Server
AgentBMCPSStack(app, "AgentBMCPSStack",
    env=env,
)

# Deploy Agent A MCP Server
AgentAMCPServerStack(app, "AgentAMCPServerStack",
    env=env,
)

# Deploy Agent A MCP Client
AgentAMCPClientStack(app, "AgentAMCPClientStack",
    env=env,
)

app.synth()