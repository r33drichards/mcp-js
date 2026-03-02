# Running SWE-bench with mcp-js and AWS Bedrock

Step-by-step guide to evaluating an AI agent on SWE-bench using the mcp-js (V8 JavaScript) runtime and AWS Bedrock as the LLM provider.

## Prerequisites

- Linux x86_64 machine (Docker required)
- Python 3.10+
- Docker installed and running
- AWS credentials with Bedrock access to Claude models
- ~120 GB free disk space, 16 GB RAM, 8+ CPU cores recommended

## Step 1: Install mcp-v8

Download and install the mcp-js server binary:

```bash
curl -fsSL https://raw.githubusercontent.com/r33drichards/mcp-js/main/install.sh | sudo bash
```

This installs the `mcp-v8` binary to `/usr/local/bin/mcp-v8`.

Verify the installation:

```bash
mcp-v8 --help
```

Set the environment variable so mini-swe-agent can find it:

```bash
export MCP_JS_BINARY=$(which mcp-v8)
```

## Step 2: Install mini-swe-agent

```bash
pip install mini-swe-agent
```

Or install from source:

```bash
git clone https://github.com/SWE-agent/mini-swe-agent.git
cd mini-swe-agent
pip install -e .
```

## Step 3: Install SWE-bench

```bash
pip install swebench
```

Or install from source:

```bash
git clone https://github.com/princeton-nlp/SWE-bench.git
cd SWE-bench
pip install -e .
```

## Step 4: Configure AWS credentials

Set your AWS credentials for Bedrock access:

```bash
export AWS_ACCESS_KEY_ID="your-access-key"
export AWS_SECRET_ACCESS_KEY="your-secret-key"
export AWS_REGION_NAME="us-east-1"  # or us-west-2
```

Or use an AWS profile:

```bash
export AWS_PROFILE="your-profile"
```

Make sure your AWS account has Bedrock model access enabled for Claude in the target region.

## Step 5: Run the agent on SWE-bench

The config file `swebench_mcp_js.yaml` is bundled with mini-swe-agent. It uses:
- **Environment**: `mcp_js_docker` — runs mcp-v8 inside the SWE-bench Docker container
- **Model**: `bedrock/us.anthropic.claude-sonnet-4-5-20250929-v1:0` (Claude Sonnet 4.5 via Bedrock)
- **Language**: JavaScript (the agent writes JS code instead of bash)

### Run on SWE-bench Verified (full dataset)

```bash
mini-extra swebench \
  --config swebench_mcp_js \
  --subset verified \
  --split test \
  --workers 4 \
  -o ./results/
```

### Run on a slice (e.g., first 5 instances)

```bash
mini-extra swebench \
  --config swebench_mcp_js \
  --subset verified \
  --split test \
  --slice 0:5 \
  --workers 2 \
  -o ./results/
```

### Run a single instance (for debugging)

```bash
mini-extra swebench-single \
  --config swebench_mcp_js \
  --subset verified \
  --split test \
  --instance sympy__sympy-20590
```

### Override the model from the command line

```bash
mini-extra swebench \
  --config swebench_mcp_js \
  --subset verified \
  --split test \
  --model "bedrock/us.anthropic.claude-opus-4-20250514-v1:0" \
  --workers 2 \
  -o ./results/
```

## Step 6: Check results

After the run completes, the output directory contains:

```
results/
├── preds.json                          # Predictions (SWE-bench format)
├── exit_statuses_<timestamp>.yaml      # Exit status per instance
├── minisweagent.log                    # Agent logs
└── <instance_id>/
    └── <instance_id>.traj.json         # Full trajectory per instance
```

`preds.json` contains one entry per instance:

```json
{
  "sympy__sympy-20590": {
    "instance_id": "sympy__sympy-20590",
    "model_name_or_path": "bedrock/us.anthropic.claude-sonnet-4-5-20250929-v1:0",
    "model_patch": "diff --git a/..."
  }
}
```

## Step 7: Evaluate with SWE-bench

### Option A: Local evaluation

```bash
python -m swebench.harness.run_evaluation \
  --dataset_name princeton-nlp/SWE-bench_Verified \
  --predictions_path ./results/preds.json \
  --max_workers 8 \
  --run_id mcp-js-eval
```

Results appear in `logs/run_evaluation/mcp-js-eval/` and a summary report is printed to stdout.

### Option B: Cloud evaluation (sb-cli)

```bash
pip install sb-cli
sb login
sb submit --predictions ./results/preds.json
```

## How it works

1. For each SWE-bench instance, mini-swe-agent pulls the corresponding Docker image (e.g., `swebench/sweb.eval.x86_64.sympy_1776_sympy-20590:latest`)
2. The mcp-v8 binary is copied into the container and started in stateless HTTP mode
3. The agent (Claude via Bedrock) receives the issue description and interacts with the codebase through JavaScript code blocks
4. Instead of bash, the agent uses `fs.*` APIs to read/write files and `fetch()` for network access
5. When the agent submits, `git diff --cached` captures the patch
6. The patch is saved to `preds.json` in standard SWE-bench format

## Troubleshooting

**"mcp-v8: command not found"**
- Make sure the binary is installed: `ls -la /usr/local/bin/mcp-v8`
- Or set `MCP_JS_BINARY` to the full path of the binary

**AWS Bedrock errors (403 / access denied)**
- Verify your AWS credentials: `aws sts get-caller-identity`
- Ensure Bedrock model access is enabled in the AWS console for your region
- Check that `AWS_REGION_NAME` matches a region where you have Claude enabled

**Docker pull failures**
- SWE-bench images are large (~2-5 GB each). Ensure sufficient disk space
- On ARM/Apple Silicon, add `--namespace ''` to build images locally

**Agent stuck or timeout**
- Default step limit is 250, cost limit is $3 per instance
- Increase `execution_timeout_secs` in the config if JavaScript executions are timing out
- Check `minisweagent.log` in the output directory for details
