#!/usr/bin/env python3
import os
import aws_cdk as cdk
from agent_b_mcp_server_stack import AgentBMCPSStack

env = cdk.Environment(
    account="940333627479",
    region="us-east-1",
)

app = cdk.App()

# env=cdk.Environment(account=os.getenv('CDK_DEFAULT_ACCOUNT'), 
# region=os.getenv('CDK_DEFAULT_REGION')),
AgentBMCPSStack(app, "AgentBMCPSStack",
    env=env,
)

app.synth()