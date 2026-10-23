# Nova Micro request/cost bounds — 2026-10-23

Runtime lane reported free CountTokens returned “The provided model doesn't support counting tokens” for us.amazon.nova-micro-v1:0. No paid model call was made by this cloud task. The chosen model is unchanged; IAM invocation approval remains separate.

## Explicit supported mode

Set BEDROCK_BUDGET_MODE=nova-request-bounds only for BEDROCK_MODEL_ID=us.amazon.nova-micro-v1:0. It skips the unsupported counting operation by explicit configuration, not by catching arbitrary permission/network errors and silently falling back. Other model IDs are rejected in this mode. Default count-tokens mode retains the previous exact counting gate for supported models.

Hard local guards apply to the default JSON UTF-8 serialization of complete Converse SDK arguments (system, history, schemas, results, model ID and inference configuration), excluding HTTP headers:

- 32,768 bytes per request.
- 98,304 cumulative submitted request bytes per manually approved run.
- 16,384 serialized bytes per tool result; oversized results stop rather than truncate.
- At most six inference attempts; maxTokens=1024 on each.
- SDK total_max_attempts=1; no automatic retries or refunds of uncertain/cancelled attempts.

Bytes are not converted to tokens. Pre-call token count is explicitly unknown; provider usage is logged after responses. Bounds are on application arguments, not an assertion about exact HTTP wire bytes or exact input tokens.

## Conservative spending planning

AWS model card documents a 128K context window: https://docs.aws.amazon.com/bedrock/latest/userguide/model-card-amazon-nova-micro.html

For planning, conservatively reserve 131,072 input tokens per attempt (larger binary interpretation of 128K), regardless of the smaller byte-bounded request. Six attempts give a loose service-context bound of 786,432 input tokens plus at most 6,144 requested output tokens. At runtime-verified prices per million tokens, conditional token-inference cost is at most:

`786432 * input_rate / 1e6 + 6144 * output_rate / 1e6`

This assumes the published model context limit and ordinary token billing, excludes infrastructure/taxes/other runs, and is not an enforced account-level USD cap. No price or exact tokenizer estimate is invented. Runtime must approve this worst-case allowance before invocation. Source model output-limit documentation varies, but the lower explicit request cap of 1024 is unchanged.

## Tests actually run

16 offline budget tests passed: nine existing CountTokens/cancellation/schema tests plus seven Nova tests covering no CountTokens call, exact measured request bytes, oversized tool schema and Unicode content, growing history, cumulative bytes, oversized tool result, rejecting another model, and cancelled-attempt reservation/no retry. Both API shapes continue to be validated against installed botocore. Five existing notebook/rehearsal tests passed. nbformat validation and seven offline notebook cells passed; delivered notebook has no outputs.

These mocks do not establish paid Nova behavior or IAM readiness. Runtime evidence from commit 910de2d is preserved unchanged. No AWS writes, credentials, paid calls, main changes or PRs were made.
