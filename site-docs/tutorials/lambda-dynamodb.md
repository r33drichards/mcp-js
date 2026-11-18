# Deploy mcp-js on Lambda with DynamoDB and S3

Run short, request-scoped JavaScript on AWS Lambda, keep metadata in DynamoDB,
and keep the virtual `/workspace` filesystem in S3. This walkthrough uses a private,
IAM-authenticated Function URL and Lambda Web Adapter in front of mcp-js.

**Validation status:** the commands below are a deployment recipe, not a record
of an executed AWS deployment. Validate the image and persistence checks in a
disposable environment before connecting an agent or GitHub event consumer.
Use a checkout containing the DynamoDB runtime changes from PR #306.

## What persists

| Storage | Contents |
| --- | --- |
| DynamoDB | Session history, filesystem labels, heap tags, execution records, console pages, artifact records and upload grants |
| S3 | Content-addressed filesystem snapshots; runtime values larger than 128 KiB; heap snapshots if separately enabled |
| Lambda `/tmp` | Disposable process-local files; never the source of truth |

`--fs-store s3` provides a **virtual filesystem through the JavaScript `fs`
API**, not an EFS mount or a POSIX volume for host subprocesses. This tutorial
selects `/workspace` through the filesystem policy; the snapshot implementation
stores absolute virtual paths and does not require the conventional `/work`
prefix. It does not attach EFS. JavaScript globals are not persisted because heap storage
is left disabled. In-flight isolates, cancellation handles, MCP transport
sessions and task subscriptions remain process-local.

```text
SigV4 client -> Function URL -> Lambda Web Adapter -> POST /mcp
                                                    |      |
                                                DynamoDB   S3
```

An existing Cloudflare/GitHub event receiver can remain outside this path. Its
Lambda client must preserve the session or snapshot handle, send `wait: true`,
and sign requests. DynamoDB persistence does not deduplicate webhook deliveries
or make an interrupted agent run resume automatically.

## Prerequisites

Use an x86_64 Linux builder with Nix, Docker, AWS CLI v2, Git, jq and Python 3.
The deployment identity needs permission to create the resources below and pass
the execution role. Authenticate through your usual AWS profile or temporary
role; do not put AWS keys in an image or repository. The Python invocation
example needs `botocore` installed in your own virtual environment.

Run shell blocks with Bash from the repository root. Choose a **new** name and
a region supporting Function URLs. These commands create billable resources.
They do not modify an existing Lambda function.

```bash
export AWS_REGION=us-west-2
export AWS_DEFAULT_REGION="$AWS_REGION"
export AWS_PAGER=''
export NAME=mcp-lambda-demo
export ACCOUNT_ID=$(aws sts get-caller-identity --query Account --output text)
export TABLE="$NAME-metadata"
export BUCKET="$NAME-$ACCOUNT_ID-$AWS_REGION"
export ROLE="$NAME-execution"
export PROBE_NAME="$NAME-recovery"
export REPOSITORY="$NAME"
export NAMESPACE=lambda-demo
export SOURCE_SHA=$(git rev-parse HEAD)
export IMAGE_TAG="source-$SOURCE_SHA"
export REGISTRY="$ACCOUNT_ID.dkr.ecr.$AWS_REGION.amazonaws.com"
```

## 1. Build a Lambda image from the exact checkout

The repository's `Runtime Lambda image` workflow packages a Nix HTTP server
closure. Its image **does not itself contain a Lambda Runtime API client**.
Do not deploy that image unchanged. Here we package the same closure and add
[Lambda Web Adapter](https://github.com/aws/aws-lambda-web-adapter), which bridges
Lambda invocations to the HTTP listener. The adapter supports non-AWS base
images and supplies the Runtime Interface Client.

```bash
nix build .#default --out-link result
mkdir -p lambda-image
nix-store -qR ./result > lambda-image/closure.txt
tar -cf lambda-image/closure.tar --files-from=lambda-image/closure.txt
SERVER_BIN="$(readlink -f result)/bin/server"
test -x "$SERVER_BIN"
sha256sum "$SERVER_BIN" > lambda-image/binary.sha256

cat > lambda-image/fs.rego <<'REGO'
package mcp.filesystem
default allow = false
allow if { input.path == "/workspace" }
allow if { startswith(input.path, "/workspace/") }
REGO

cat > lambda-image/Dockerfile <<DOCKER
FROM debian:trixie-slim
ADD closure.tar /
COPY --from=public.ecr.aws/awsguru/aws-lambda-adapter:1.1.0 /lambda-adapter /opt/extensions/lambda-adapter
COPY fs.rego /opt/mcp/fs.rego
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
ENV PORT=8080 AWS_LWA_PORT=8080 AWS_LWA_INVOKE_MODE=buffered \
    AWS_LWA_READINESS_CHECK_PATH=/api/version \
    AWS_LWA_READINESS_CHECK_HEALTHY_STATUS=200 \
    SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt
WORKDIR /tmp
LABEL org.opencontainers.image.revision="$SOURCE_SHA"
ENTRYPOINT ["$SERVER_BIN"]
CMD ["--http-port", "8080", "--session-db-path", "/tmp/mcp-v8", "--execution-timeout", "60", "--fs-store", "s3", "--policies-json", "{\"filesystem\":{\"policies\":[{\"url\":\"file:///opt/mcp/fs.rego\"}]}}"]
DOCKER

docker build --platform linux/amd64 -t "$REPOSITORY:$IMAGE_TAG" lambda-image
docker run --rm -e PORT= "$REPOSITORY:$IMAGE_TAG" --version
docker image inspect "$REPOSITORY:$IMAGE_TAG" \
  --format '{{.Architecture}} {{index .Config.Labels "org.opencontainers.image.revision"}}'
```

The version command checks the executable, not Lambda invocation compatibility.
Keep the source SHA and binary hash with your deployment record. Pin the Debian
and adapter images by reviewed digest in a production build. Build one
architecture, not a multi-platform manifest, and keep the Nix closure within
[Lambda's container image requirements](https://docs.aws.amazon.com/lambda/latest/dg/images-create.html).
Lambda provides a read-only root filesystem; writable local paths belong under
`/tmp`. Do not enable a host filesystem cache for durable data.

## 2. Create dedicated storage

The server expects a table with String keys `pk` and `sk`, with no indexes. It
does not create the table itself. A namespace separates logical records but is
not an authorization boundary. A dedicated table and bucket simplify both IAM
and cleanup.

```bash
aws dynamodb create-table --table-name "$TABLE" \
  --attribute-definitions AttributeName=pk,AttributeType=S AttributeName=sk,AttributeType=S \
  --key-schema AttributeName=pk,KeyType=HASH AttributeName=sk,KeyType=RANGE \
  --billing-mode PAY_PER_REQUEST
aws dynamodb wait table-exists --table-name "$TABLE"

if [ "$AWS_REGION" = us-east-1 ]; then
  aws s3api create-bucket --bucket "$BUCKET"
else
  aws s3api create-bucket --bucket "$BUCKET" \
    --create-bucket-configuration LocationConstraint="$AWS_REGION"
fi
aws s3api put-public-access-block --bucket "$BUCKET" \
  --public-access-block-configuration BlockPublicAcls=true,IgnorePublicAcls=true,BlockPublicPolicy=true,RestrictPublicBuckets=true
```

Keep default S3 encryption or configure your organization's encryption policy.
SSE-KMS requires additional key permissions not included below. Do not add an
expiration rule that deletes live snapshots or `runtime/` blobs: metadata may
still reference them. Runtime metadata has no automatic expiry, and filesystem
GC does not collect replaced or unreferenced runtime blobs.

## 3. Give Lambda access to its storage

The execution role uses the AWS SDK credential chain. Its policy covers this
table, this bucket's objects and this function's log group. It cannot provision
or delete tables. DynamoDB transactions authorize their constituent operations;
there is no `dynamodb:TransactWriteItems` IAM action to grant.

```bash
cat > lambda-trust.json <<'JSON'
{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"lambda.amazonaws.com"},"Action":"sts:AssumeRole"}]}
JSON
aws iam create-role --role-name "$ROLE" \
  --assume-role-policy-document file://lambda-trust.json
aws logs create-log-group --log-group-name "/aws/lambda/$NAME"
aws logs put-retention-policy --log-group-name "/aws/lambda/$NAME" --retention-in-days 7
aws logs create-log-group --log-group-name "/aws/lambda/$PROBE_NAME"
aws logs put-retention-policy --log-group-name "/aws/lambda/$PROBE_NAME" --retention-in-days 7

cat > lambda-storage-policy.json <<JSON
{
  "Version":"2012-10-17",
  "Statement":[
    {"Effect":"Allow","Action":["dynamodb:GetItem","dynamodb:PutItem","dynamodb:UpdateItem","dynamodb:Query","dynamodb:Scan"],"Resource":"arn:aws:dynamodb:$AWS_REGION:$ACCOUNT_ID:table/$TABLE"},
    {"Effect":"Allow","Action":["s3:GetObject","s3:PutObject"],"Resource":"arn:aws:s3:::$BUCKET/*"},
    {"Effect":"Allow","Action":["logs:CreateLogStream","logs:PutLogEvents"],"Resource":["arn:aws:logs:$AWS_REGION:$ACCOUNT_ID:log-group:/aws/lambda/$NAME:*","arn:aws:logs:$AWS_REGION:$ACCOUNT_ID:log-group:/aws/lambda/$PROBE_NAME:*"]}
  ]
}
JSON
aws iam put-role-policy --role-name "$ROLE" --policy-name mcp-storage \
  --policy-document file://lambda-storage-policy.json
export ROLE_ARN=$(aws iam get-role --role-name "$ROLE" --query Role.Arn --output text)
```

Allow IAM propagation before creating the function; if Lambda reports that it
cannot assume the role, wait and retry that creation command. The invocation
identity is separate from this execution role. Do not expose execution-role
credentials to JavaScript or grant the event receiver broad storage access.

## 4. Publish to ECR and create the function

```bash
aws ecr create-repository --repository-name "$REPOSITORY" --image-tag-mutability IMMUTABLE
aws ecr get-login-password | docker login --username AWS --password-stdin "$REGISTRY"
docker tag "$REPOSITORY:$IMAGE_TAG" "$REGISTRY/$REPOSITORY:$IMAGE_TAG"
docker push "$REGISTRY/$REPOSITORY:$IMAGE_TAG"
export IMAGE_DIGEST=$(aws ecr describe-images --repository-name "$REPOSITORY" \
  --image-ids imageTag="$IMAGE_TAG" --query 'imageDetails[0].imageDigest' --output text)
export IMAGE_URI="$REGISTRY/$REPOSITORY@$IMAGE_DIGEST"

jq -n --arg table "$TABLE" --arg namespace "$NAMESPACE" --arg bucket "$BUCKET" \
  '{Variables:{MCP_V8_DYNAMODB_TABLE:$table,MCP_V8_DYNAMODB_NAMESPACE:$namespace,MCP_V8_S3_BUCKET:$bucket,MCP_V8_ALLOWED_HOSTS:"localhost,127.0.0.1"}}' \
  > lambda-env.json
aws lambda create-function --function-name "$NAME" --package-type Image \
  --code ImageUri="$IMAGE_URI" --role "$ROLE_ARN" --architectures x86_64 \
  --timeout 360 --memory-size 2048 --ephemeral-storage Size=1024 \
  --environment file://lambda-env.json
aws lambda wait function-active-v2 --function-name "$NAME"
aws lambda put-function-concurrency --function-name "$NAME" --reserved-concurrent-executions 1
aws lambda create-function-url-config --function-name "$NAME" \
  --auth-type AWS_IAM --invoke-mode BUFFERED
export FUNCTION_URL=$(aws lambda get-function-url-config --function-name "$NAME" \
  --query FunctionUrl --output text)
FUNCTION_HOST=$(python3 -c 'import os,urllib.parse; print(urllib.parse.urlsplit(os.environ["FUNCTION_URL"]).hostname)')
jq --arg host "$FUNCTION_HOST" '.Variables.MCP_V8_ALLOWED_HOSTS=("localhost,127.0.0.1,"+$host)' \
  lambda-env.json > lambda-env-ready.json
aws lambda update-function-configuration --function-name "$NAME" --environment file://lambda-env-ready.json
aws lambda wait function-updated-v2 --function-name "$NAME"
```

The [image function API](https://docs.aws.amazon.com/cli/latest/reference/lambda/create-function.html)
uses an ECR URI and execution role; no handler or managed runtime is selected.
Use ECR in the function's region and ensure the repository permits Lambda image
retrieval. Reserved concurrency limits this demo to one request at a time; it
does not make `/tmp` durable or guarantee the same execution environment.

The adapter's buffered mode matches the
[Function URL mode](https://docs.aws.amazon.com/cli/latest/reference/lambda/create-function-url-config.html).
Do not open a long-lived SSE connection. Use the server's session-less MCP
protocol (`2026-07-28`) so requests can reach fresh processes. Clients using
older transport sessions need a separate compatibility assessment.

Give the **caller** an identity policy allowing `lambda:InvokeFunctionUrl` and
`lambda:InvokeFunction` on `arn:aws:lambda:REGION:ACCOUNT:function:NAME`, scoped
to this function. For `InvokeFunction`, use the condition
`"Bool":{"lambda:InvokedViaFunctionUrl":"true"}`. Same-account callers with
both identity permissions need no public resource policy; cross-account callers
also need an appropriate function resource policy. See
[Function URL access control](https://docs.aws.amazon.com/lambda/latest/dg/urls-auth.html).
Never switch to `NONE` just to make an unsigned MCP client connect. This demo
trusts authorized IAM callers; add application authentication and tenant policy
before offering it to mutually untrusted users.

## 5. Invoke with `wait: true`

Save this as `invoke-mcp.py`. It uses your configured credential provider and
[SigV4-signs the Function URL request](https://docs.aws.amazon.com/lambda/latest/dg/urls-invocation.html)
without printing credentials. It accepts a tool name and JSON arguments. The
session header is an application session key, not an MCP transport session.

```python
import json
import os
import sys
import urllib.request

import botocore.session
from botocore.auth import SigV4Auth
from botocore.awsrequest import AWSRequest

tool, arguments = sys.argv[1], json.loads(sys.argv[2])
body = json.dumps({
    "jsonrpc": "2.0", "id": 1, "method": "tools/call",
    "params": {"name": tool, "arguments": arguments, "_meta": {
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {}
    }}
}).encode()
url = os.environ["FUNCTION_URL"].rstrip("/") + "/mcp"
headers = {
    "Content-Type": "application/json",
    "Accept": "application/json, text/event-stream",
    "MCP-Protocol-Version": "2026-07-28",
    "Mcp-Method": "tools/call", "Mcp-Name": tool,
    "X-MCP-Session-Id": "lambda-persistence-proof"
}
session = botocore.session.get_session()
credentials = session.get_credentials().get_frozen_credentials()
request = AWSRequest(method="POST", url=url, data=body, headers=headers)
SigV4Auth(credentials, "lambda", os.environ["AWS_REGION"]).add_auth(request)
with urllib.request.urlopen(urllib.request.Request(
    url, data=body, headers=dict(request.headers), method="POST"
), timeout=370) as response:
    text = response.read().decode()
raw = next((line[6:] for line in text.splitlines() if line.startswith("data: ")), text)
rpc = json.loads(raw)
if "error" in rpc:
    raise RuntimeError(rpc["error"])
result = rpc["result"]
if result.get("isError"):
    raise RuntimeError(result)
print(json.dumps(json.loads(result["content"][0]["text"]), indent=2))
```

```bash
python3 invoke-mcp.py run_js \
  '{"wait":true,"code":"await fs.writeFile(\"/workspace/proof.txt\", \"survived\"); console.log(\"stored\");"}' \
  > write-result.json
jq -e '.status == "completed" and (.fs | length == 64)' write-result.json
export EXECUTION_ID=$(jq -r .execution_id write-result.json)
export SNAPSHOT_ID=$(jq -r .fs write-result.json)
```

The reply acknowledges execution and snapshot persistence. Keep the execution
ID and snapshot ID. `wait` is capped at 305 seconds; a `wait_timed_out` response
or lost HTTP reply is an uncertain outcome, not permission to replay a write.
Keep the execution deadline below the Lambda and client deadlines. Inspect
`output_truncated` and use `get_execution_output` for additional output. Large
responses must also fit the buffered Function URL payload limit.

## 6. Prove recovery in a different Lambda process

Create a second function from the same image, execution role and environment,
using the same table, namespace and bucket. A different function cannot reuse
the first function's process or `/tmp`, making this a stronger check than two
calls that happen to hit one warm instance.

```bash
export PROBE_NAME="$NAME-recovery"
aws lambda create-function --function-name "$PROBE_NAME" --package-type Image \
  --code ImageUri="$IMAGE_URI" --role "$ROLE_ARN" --architectures x86_64 \
  --timeout 360 --memory-size 2048 --ephemeral-storage Size=1024 \
  --environment file://lambda-env.json
aws lambda wait function-active-v2 --function-name "$PROBE_NAME"
aws lambda create-function-url-config --function-name "$PROBE_NAME" \
  --auth-type AWS_IAM --invoke-mode BUFFERED
export FUNCTION_URL=$(aws lambda get-function-url-config --function-name "$PROBE_NAME" \
  --query FunctionUrl --output text)
PROBE_HOST=$(python3 -c 'import os,urllib.parse; print(urllib.parse.urlsplit(os.environ["FUNCTION_URL"]).hostname)')
jq --arg host "$PROBE_HOST" '.Variables.MCP_V8_ALLOWED_HOSTS=("localhost,127.0.0.1,"+$host)' \
  lambda-env.json > lambda-env-probe.json
aws lambda update-function-configuration --function-name "$PROBE_NAME" --environment file://lambda-env-probe.json
aws lambda wait function-updated-v2 --function-name "$PROBE_NAME"
python3 invoke-mcp.py get_execution "$(jq -nc --arg id "$EXECUTION_ID" '{execution_id:$id}')"
python3 invoke-mcp.py get_execution_output "$(jq -nc --arg id "$EXECUTION_ID" '{execution_id:$id}')"
python3 invoke-mcp.py run_js \
  '{"wait":true,"code":"console.log(await fs.readFile(\"/workspace/proof.txt\", \"utf8\"));"}'
```

Grant the caller the same two invoke permissions on the recovery function before
calling it.
Expect the old execution to be `completed`, its console to contain `stored`,
and the new execution's output to contain `survived`. The last call uses the
same application session header to recover its filesystem pointer. Also test
an explicit `fs` argument containing `SNAPSHOT_ID` when integrating a client
that carries snapshot handles instead of session headers.

Before promotion, repeat with a large artifact to exercise the S3 runtime-blob
path, verify permission failures are surfaced, and test the actual agent's
request protocol. The small file above does not prove large-artifact recovery,
crash recovery for interrupted code, or webhook exactly-once processing.

## Limits, rollout and cleanup

For an **existing OAuth-protected Lambda deployment**, preserve its Function URL,
adapter, JWT/JWKS enforcement, issuer/audience validation and client secret flow.
Do not replace its environment with this demo's `lambda-env.json`: Lambda's
environment update replaces the whole map. Merge the DynamoDB table/namespace,
S3 bucket and `--fs-store s3` settings into its declarative configuration, retain
its filesystem policy and add the storage permissions to its existing execution
role. Keep the client on `wait: true` and stable session IDs. Validate a separate
canary against the same authentication contract before promoting the image;
using new storage starts with empty state unless you explicitly migrate old data.
To adopt `/workspace`, update the allowlist and the agent's file paths together.
Existing `/work` snapshot entries are not renamed automatically. Preserve them
until an explicit copy/migration has been verified. Keep Lambda process scratch
and `--session-db-path` under `/tmp`; moving those directories to a local
`/workspace` would not provide persistence.

- DynamoDB mode cannot be combined with Raft `--cluster-port`. Existing sled
  data is not migrated automatically. Scheduler checkpoints and HTTP-function
  definitions are not made durable by this flag.
- Completed execution records survive process replacement. Running JavaScript
  and cross-process cancellation do not. Do not use fire-and-forget execution,
  in-process schedulers or long-lived subscriptions as durable Lambda jobs.
- Serialize writes within an application session, or use filesystem labels
  with compare-and-swap and handle conflicts. Concurrency limits on one function
  do not serialize calls made through another function.
- Lambda duration/memory, DynamoDB requests (including discovery scans), S3
  requests/storage, ECR storage and logs cost money. The demo allocates no
  provisioned concurrency and uses on-demand DynamoDB. Monitor usage and remove
  the recovery function after the checks.
- Promote a verified image digest through your existing deployment system.
  Preserve the prior image digest and configuration for rollback. Do not enable
  GitHub event delivery until the agent's invocation and persistence checks
  pass. This tutorial does not change a Cloudflare Worker or webhook.

For this disposable demo only, after inspecting the names and saving anything
you need, delete its resources. Bucket removal below destroys all demo data.
For an existing deployment, retain storage and follow its migration/retention
policy instead.

```bash
aws lambda delete-function-url-config --function-name "$PROBE_NAME"
aws lambda delete-function --function-name "$PROBE_NAME"
aws lambda delete-function-url-config --function-name "$NAME"
aws lambda delete-function --function-name "$NAME"
aws dynamodb delete-table --table-name "$TABLE"
aws s3 rb "s3://$BUCKET" --force
aws ecr delete-repository --repository-name "$REPOSITORY" --force
aws logs delete-log-group --log-group-name "/aws/lambda/$NAME"
aws logs delete-log-group --log-group-name "/aws/lambda/$PROBE_NAME"
aws iam delete-role-policy --role-name "$ROLE" --policy-name mcp-storage
aws iam delete-role --role-name "$ROLE"
```

Remove any caller policies you added for the two functions. A versioned bucket
also requires deletion of object versions and delete markers; this recipe does
not enable versioning.

See [filesystem snapshots](../how-to/fs-snapshots.md),
[MCP tools](../reference/mcp-tools.md), and
[authentication](../how-to/authentication.md) for the server-side contracts.
