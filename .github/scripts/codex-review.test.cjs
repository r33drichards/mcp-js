const { test } = require('node:test');
const assert = require('node:assert/strict');
const run = require('./codex-review.cjs');
const bot = { login: 'chatgpt-codex-connector[bot]', type: 'Bot' };
function fixture({ reviews = [], reactions = [], permission = 'write', changeHead = false } = {}) {
  const comments = [], removed = [];
  let reads = 0;
  const pr = () => ({ state: 'open', labels: [{ name: 'review' }],
    head: { sha: changeHead && ++reads > 1 ? 'new-head' : 'head' } });
  const listReviews = () => {};
  const github = { rest: {
    repos: { getCollaboratorPermissionLevel: async () => ({ data: { permission } }) },
    pulls: { get: async () => ({ data: pr() }), listReviews },
    reactions: { listForIssueComment: () => {} },
    issues: { createComment: async (comment) => { comments.push(comment); return {
      data: { id: 42, created_at: '2026-10-25T00:00:00Z' },
    }; }, removeLabel: async (label) => removed.push(label) },
  }, paginate: async (method) => method === listReviews ? reviews : reactions };
  return { comments, removed, args: { github, requester: github,
    context: { repo: { owner: 'owner', repo: 'repo' }, actor: 'writer', runId: 1,
      payload: { pull_request: { number: 7 } } }, core: { info() {} },
    sleep: async () => {}, attempts: 1 } };
}
const review = { user: bot, commit_id: 'head', state: 'COMMENTED', submitted_at: '2026-10-25T00:00:01Z' };
test('requests review, then removes label for a new review on the requested commit', async () => {
  const f = fixture({ reviews: [review] }); await run(f.args);
  assert.match(f.comments[0].body, /^@codex review/); assert.match(f.comments[0].body, /review is starting/);
  assert.equal(f.removed.length, 1); assert.equal(f.removed[0].name, 'review');
});
test('recognizes Codex no-findings thumbs-up on the request', async () => {
  const f = fixture({ reactions: [{ user: bot, content: '+1' }] }); await run(f.args);
  assert.equal(f.removed.length, 1);
});
for (const [name, options] of [
  ['old review', { reviews: [{ ...review, submitted_at: '2026-10-24T00:00:00Z' }] }],
  ['wrong commit', { reviews: [{ ...review, commit_id: 'old-head' }] }],
  ['pending review', { reviews: [{ ...review, state: 'PENDING' }] }],
  ['human review', { reviews: [{ ...review, user: { login: bot.login, type: 'User' } }] }],
  ['human thumbs-up', { reactions: [{ user: { login: 'writer', type: 'User' }, content: '+1' }] }],
  ['eyes only', { reactions: [{ user: bot, content: 'eyes' }] }],
  ['no response', {}],
]) test(`retains label on ${name}`, async () => {
  const f = fixture(options); await assert.rejects(run(f.args), /No native Codex completion/);
  assert.equal(f.removed.length, 0);
});
test('rejects unauthorized actor before posting a request', async () => {
  const f = fixture({ permission: 'read' }); await assert.rejects(run(f.args), /write access/);
  assert.equal(f.comments.length, 0);
});
test('retains label when the PR head changes', async () => {
  const f = fixture({ changeHead: true, reviews: [review] }); await assert.rejects(run(f.args), /head changed/);
  assert.equal(f.removed.length, 0);
});
test('fails without a user token', async () => {
  const f = fixture(); f.args.requester = null; await assert.rejects(run(f.args), /CODEX_REVIEW_REQUEST_TOKEN/);
  assert.equal(f.comments.length, 0);
});
