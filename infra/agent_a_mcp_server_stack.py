from aws_cdk import (
    Stack,
    CfnOutput,
    RemovalPolicy,
    aws_ecr as ecr,
    aws_ecr_assets as ecr_assets,
)
from constructs import Construct

from cdk_ecr_deployment import ECRDeployment, DockerImageName


class AgentAMCPServerStack(Stack):

    def __init__(self, scope: Construct, construct_id: str, **kwargs) -> None:
        super().__init__(scope, construct_id, **kwargs)

        # Create your own permanent ECR repository
        repo = ecr.Repository(
            self,
            "AgentAMCPServerRepo",
            repository_name="agent-a-mcp-server",  # lowercase, as required by ECR
            image_tag_mutability=ecr.TagMutability.MUTABLE,
            removal_policy=RemovalPolicy.RETAIN,  # Safe default – keeps images on cdk destroy
        )

        # Build the Docker image asset (CDK pushes to temporary asset repo)
        docker_image = ecr_assets.DockerImageAsset(
            self,
            "AgentAMCPServerImage",
            directory="..",
            file="agent-a/mcp-server/Dockerfile",
            exclude=["cdk.out", "infra/cdk.out", ".git", ".gitignore", "node_modules", "*.md"],
        )

        # Get the desired tag from CDK context (passed from GitHub Actions)
        dest_tag = self.node.try_get_context("image_tag") or "latest"

        # Copy the image to your custom repo with the correct tag
        deployment = ECRDeployment(
            self,
            "DeployAgentAMCPServerImage",
            src=DockerImageName(docker_image.image_uri),
            dest=DockerImageName(f"{repo.repository_uri_for_tag(dest_tag)}"),
        )

        # Ensure the repo is created first
        deployment.node.add_dependency(repo)

        # Output the clean, human-readable tagged URI
        CfnOutput(
            self,
            "ImageUri",
            value=f"{repo.repository_uri}:{dest_tag}",
            description="ECR Image URI with custom tag (agent-a-mcp-server:latest or agent-a-mcp-server:vX.Y.Z)",
        )
