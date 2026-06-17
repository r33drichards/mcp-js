# Welcome to MCP-v8 Documentation

The Model Context Protocol (MCP) is a standard for enabling communication between AI models and applications. This documentation helps you understand and use the JavaScript implementation of MCP.

## What is MCP?

The Model Context Protocol (MCP) provides a standardized way for AI models to communicate with applications, enabling features like:

- Tool execution
- Context management  
- Secure communication
- Cross-platform compatibility

## Getting Started

Whether you're looking to:
- Run existing examples
- Build your own MCP server
- Integrate with existing systems
- Understand the architecture

This documentation is organized to help you accomplish these goals efficiently.

## Documentation Structure

Our documentation follows the [Diátaxis framework](https://diataxis.fr/), which organizes information into four modes:

1. **Tutorials** - Learning-oriented, hands-on experience
2. **How-to guides** - Task-oriented instructions  
3. **Explanation** - Conceptual understanding
4. **Reference** - Factual information and API documentation

Choose the section that best matches your current needs.
EOF && cat > site-docs/tutorials/overview.md << 'EOF'
# Tutorials Overview

Tutorials are learning-oriented guides that take you through a practical experience under the guidance of a tutor. They're designed to help you learn by doing something meaningful towards an achievable goal.

## What You'll Learn

- How to set up your development environment
- How to run basic examples
- Core concepts through hands-on practice
- Step-by-step implementation of features

## Prerequisites

Before starting tutorials, make sure you have:

- Node.js installed (version 16 or higher)
- A basic understanding of JavaScript/TypeScript
- Familiarity with command-line tools
EOF && echo "Created basic documentation files" && mkdocs build --strict