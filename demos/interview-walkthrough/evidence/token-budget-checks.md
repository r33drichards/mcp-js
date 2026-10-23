# Full-input token budget checks — 2026-10-23

Changed the notebook's pre-inference cost gate. The 8,192-character alert limit remains only an intake size check. Bedrock CountTokens now counts the exact model ID, system block, tool schemas and complete history including tool responses before each Converse call. The cumulative run budget is 8,192 input tokens total, not per turn. A separate 120,000-byte full-input guard is explicitly not a token estimate.

Reserve before inference; do not refund uncertain/cancelled attempts. CountTokens and Converse use the existing async client with total_max_attempts=1, so there are no SDK retries. Maximum six inference calls and 1,024 requested output tokens per call. Counting failure, unsupported model or invalid response stops before inference. No silent model change or truncation. Dollar cost still depends on verified model pricing; a manually approved new run has a separate budget.

Nine offline tests passed in test_token_budget.py:

1. CountTokens payload equals the actual full Converse input, including the second turn's tool result; both pass installed botocore schema validation.
2. First full input above 8,192 tokens sends no inference.
3. Repeated history makes cumulative 5,000 + 4,000 exceed the budget; only first inference executes.
4. Oversized tool output stops before another count/inference.
5. Oversized system instructions or schemas stop before counting.
6. Unsupported CountTokens fails closed.
7. Cancellation during counting sends no inference.
8. Cancellation during inference retains reservation and does not retry.
9. Six-call and 1,024-output-token limits are preserved.

The five existing notebook/rehearsal tests also passed. These are explicit offline mocks, not AWS inference evidence. No paid calls or AWS operations were made. Selected-model CountTokens support/permission and final Bedrock end-to-end test remain runtime-lane responsibilities. Prior deterministic runtime success was reported by that lane; this test report does not independently reproduce it.

References: https://docs.aws.amazon.com/bedrock/latest/APIReference/API_runtime_CountTokens.html and https://docs.aws.amazon.com/bedrock/latest/APIReference/API_runtime_ConverseTokensRequest.html
