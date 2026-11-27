const BOT = 'chatgpt-codex-connector[bot]';
const isCodex = (user) => user?.type === 'Bot' && user.login === BOT;

module.exports = async function run({ github, requester, context, core,
  sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms)), attempts = 120 }) {
  if (!requester) throw new Error('Set CODEX_REVIEW_REQUEST_TOKEN to a GitHub user token; see docs/codex-review.md.');
  const repo = context.repo;
  const issue_number = context.payload.pull_request.number;
  const pull = { ...repo, pull_number: issue_number };
  const issue = { ...repo, issue_number };
  const { data: permission } = await github.rest.repos.getCollaboratorPermissionLevel({
    ...repo, username: context.actor,
  });
  if (!['admin', 'maintain', 'write'].includes(permission.permission)) {
    throw new Error('Only users with repository write access may request reviews.');
  }
  const { data: pr } = await github.rest.pulls.get(pull);
  if (pr.state !== 'open' || !pr.labels.some((label) => label.name === 'review')) return;
  const head = pr.head.sha;
  const { data: request } = await requester.rest.issues.createComment({
    ...issue,
    body: `@codex review\n\nCodex review is starting: requested for commit ${head}. ` +
      'Follow the Review guidelines in AGENTS.md, including code quality, security, ' +
      'performance, testing, documentation, and applicable compliance, financial, and e-commerce checks. ' +
      'Use inline comments for specific findings and a top-level summary for general observations.\n\n' +
      `<!-- codex-label-review:${context.runId}:${head} -->`,
  });
  // Native Codex signals findings via a submitted review, or no findings via a
  // thumbs-up reaction on the request. Eyes/replies/errors are not completion.
  for (let i = 0; i < attempts; i++) {
    const { data: current } = await github.rest.pulls.get(pull);
    if (current.state !== 'open' || current.head.sha !== head) {
      throw new Error('PR closed or head changed; retaining review label. Remove and reapply it to review the latest commit.');
    }
    if (!current.labels.some((label) => label.name === 'review')) return;
    const reviews = await github.paginate(github.rest.pulls.listReviews, { ...pull, per_page: 100 });
    const reactions = await github.paginate(github.rest.reactions.listForIssueComment, {
      ...repo, comment_id: request.id, per_page: 100,
    });
    const completed = reviews.some((review) => isCodex(review.user) &&
      review.commit_id === head && ['COMMENTED', 'APPROVED', 'CHANGES_REQUESTED'].includes(review.state) &&
      Date.parse(review.submitted_at) >= Date.parse(request.created_at)) ||
      reactions.some((reaction) => isCodex(reaction.user) && reaction.content === '+1');
    if (completed) {
      // Check again before changing labels; a newer commit requires a new review.
      const { data: latest } = await github.rest.pulls.get(pull);
      if (latest.head.sha !== head || latest.state !== 'open') throw new Error('PR changed before completion; retaining review label.');
      try {
        await github.rest.issues.removeLabel({ ...issue, name: 'review' });
      } catch (error) {
        if (error.status !== 404) throw error;
      }
      core.info(`Native Codex review completed for ${head}; removed review label.`);
      return;
    }
    await sleep(15000);
  }
  throw new Error('No native Codex completion signal within 30 minutes; retaining review label. Check Codex access, usage limits, and the request comment.');
};
