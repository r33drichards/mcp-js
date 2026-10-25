# Codex reviews with a ChatGPT subscription

Adding the `review` label to an open PR requests a native `@codex review`. The
request comment announces the review and references the criteria in the root
`AGENTS.md`. Native Codex posts findings; GitHub Actions watches for completion
and removes the label. This workflow makes no OpenAI API calls and requires no
OpenAI API key. Codex reviews use the connected ChatGPT account's review allowance;
GitHub Actions runtime is subject to GitHub billing and limits.

## Setup

1. Sign in at <https://chatgpt.com/codex> with your subscribed ChatGPT account,
   connect GitHub, and enable code review for `r33drichards/mcp-js`. Keep automatic
   reviews disabled if reviews should run only on request.
2. Create a fine-grained **GitHub user** personal access token owned by the user
   who has Codex review access. Limit repository access to `mcp-js`, grant
   **Issues: read and write** and **Pull requests: read and write**, and use an
   expiration date. Store it as the repository Actions secret
   `CODEX_REVIEW_REQUEST_TOKEN`. This is a GitHub credential, not a ChatGPT token.
   The request must come from an eligible user; the default Actions bot token is
   not a substitute. Never copy ChatGPT cookies or Codex authentication files
   into GitHub secrets.
3. Merge the workflow, script, and guidelines into `main`. Create the `review`
   label if needed, then apply it to an open test PR as a repository writer.
4. Verify that the user-authored request appears, Codex responds, and the label
   disappears after completion. This integration needs a live smoke test:
   account eligibility and handling of automated user-token requests are external
   service behavior, not established by the local tests.

## Completion and retries

Completion means a submitted review from `chatgpt-codex-connector[bot]` for the
requested commit after the request time, or that bot's thumbs-up reaction on the
request comment. An eyes reaction, an arbitrary comment, a usage-limit error,
missing access, or silence does not count. Native Codex controls the exact format
and breadth of its output; guidelines request inline findings and a summary but
cannot guarantee exhaustive coverage or legal compliance.

The workflow waits up to 30 minutes. If it fails, times out, or sees a changed PR
head, it retains the label. Inspect the Actions log and Codex request, resolve the
cause, then remove and reapply `review`. Re-running a failed Actions job also
posts a new request and may consume another review allowance. Removing the label
stops monitoring on the next poll but does not cancel a native review already
requested. New commits require a new label request. Avoid overlapping manual
reviews on the same commit because native reviews do not expose a request ID.

The job uses `pull_request_target` for fork PR support but checks out only the
trusted base commit. It never builds, executes, or checks out PR-head code, and
restricts requests to repository writers. Secrets stay out of prompts and logs.
Changes to the workflow and its script require careful maintainer review.

Local verification: `node --test .github/scripts/codex-review.test.cjs`.
See [native Codex code review documentation](https://developers.openai.com/codex/cloud/code-review).
