const test = require('node:test');
const assert = require('node:assert/strict');
const path = require('node:path');
const { parseHTML } = require('linkedom');
const { window } = parseHTML('<!doctype html><html><body></body></html>');
window.document.oninput = null;
for (const name of ['window', 'document', 'HTMLElement', 'HTMLInputElement', 'HTMLTextAreaElement', 'HTMLSelectElement', 'Event', 'MouseEvent', 'Node']) if (window[name]) Object.defineProperty(globalThis, name, { value: window[name], configurable: true, writable: true });
Object.defineProperty(globalThis, 'navigator', { value: { userAgent: 'Linkedom cloud integration test' }, configurable: true });
globalThis.IS_REACT_ACT_ENVIRONMENT = true;
window.confirm = () => true;
const React = require('react');
const { act } = React;
const { createRoot } = require('react-dom/client');
const { renderToStaticMarkup } = require('react-dom/server');
const source = require('esbuild').buildSync({ entryPoints: [path.join(__dirname, '../src/CloudPages.jsx')], bundle: true, write: false, platform: 'node', format: 'cjs', external: ['react', 'react-dom', 'react-feather'], loader: { '.css': 'empty' } }).outputFiles[0].text;
const moduleScope = { exports: {} };
new Function('require', 'module', 'exports', source)(require, moduleScope, moduleScope.exports);
const { CloudPage, AgentPage, IntegrationsPage, proposalPatch, calendarDraft } = moduleScope.exports;
const cloud = { serverUrl: 'http://127.0.0.1:4791', configured: true, authenticated: true, user: { id: 'me', name: 'Ada', email: 'ada@example.com' }, teamId: 't1', teams: [{ id: 't1', name: 'Studio', role: 'owner' }], conflicts: [] };
const state = { entries: [{ id: 'e1', description: 'Reviewed design', start: '2026-10-17T10:00:00.000Z', end: '2026-10-17T10:30:00.000Z', status: 'approved', clientId: '', projectId: '', taskId: '', title: 'Private window title' }, { id: 'pending', description: 'Unreviewed private activity', status: 'pending' }], clients: [{ id: 'c1', name: 'Studio client' }], projects: [{ id: 'p1', name: 'Launch', clientId: 'c1' }], tasks: [] };
const responses = { 'services.status': { ai: { configured: true, embeddingConfigured: true, model: 'configured-model' }, secretStorageConfigured: true }, 'ai.skills.list': { skills: [] }, 'ai.routines.list': { routines: [] }, 'ai.reports.list': { reports: [] }, 'integrations.list': { integrations: [] }, 'team.members': { members: [] } };
function findButton(container, text) { const found = [...container.querySelectorAll('button')].find(item => item.textContent.trim() === text); assert.ok(found, `Button ${text} should exist`); return found; }
async function click(element) { await act(async () => element.dispatchEvent(new window.Event('click', { bubbles: true }))); }
async function submit(form) { await act(async () => form.dispatchEvent(new window.Event('submit', { bubbles: true, cancelable: true }))); }
async function input(element, value) { Object.getOwnPropertyDescriptor(Object.getPrototypeOf(element), 'value').set.call(element, value); await act(async () => element.dispatchEvent(new window.Event('input', { bubbles: true }))); }
async function check(element, value) { element.checked = value; await click(element); }
async function select(element, value) { for (const option of element.options) option.selected = option.value === value; await act(async () => element.dispatchEvent(new window.Event('change', { bubbles: true }))); }
async function mount(Component, overrides = {}) {
  const calls = [], commands = [];
  const container = document.createElement('div'); document.body.appendChild(container);
  const root = createRoot(container);
  const props = { state, cloud, mutate: async command => { commands.push(command); }, cloudRequest: async (action, payload) => { calls.push({ action, payload }); return responses[action] || {}; }, ...overrides };
  await act(async () => root.render(React.createElement(Component, props)));
  return { container, calls, commands, cleanup: async () => { await act(async () => root.unmount()); container.remove(); } };
}

test('cloud page never synchronizes on mount and requires explicit consent', async () => {
  const app = await mount(CloudPage);
  try {
    const sync = findButton(app.container, 'Sync approved workspace');
    assert.equal(sync.disabled, true); assert.equal(app.calls.some(call => call.action === 'sync'), false);
    await check(app.container.querySelector('.cp-consent input'), true);
    assert.equal(sync.disabled, false);
    await click(sync);
    assert.deepEqual(app.calls.find(call => call.action === 'sync').payload, { consent: true, resolutions: [] });
  } finally { await app.cleanup(); }
});

test('sync conflicts stay unresolved until the user chooses a version', async () => {
  const conflict = { kind: 'entries', id: 'e1', local: { description: 'Local draft' }, remote: { data: { description: 'Server edit' } } };
  const app = await mount(CloudPage, { cloud: { ...cloud, conflicts: [conflict] } });
  try {
    assert.match(app.container.textContent, /Local draft/); assert.match(app.container.textContent, /Server edit/);
    assert.equal(app.calls.some(call => call.action === 'sync'), false);
    await select(app.container.querySelector('[aria-label="Resolve entries e1"]'), 'remote');
    await check(app.container.querySelector('.cp-consent input'), true);
    await click(findButton(app.container, 'Sync approved workspace'));
    assert.deepEqual(app.calls.find(call => call.action === 'sync').payload.resolutions, [{ kind: 'entries', id: 'e1', strategy: 'remote' }]);
  } finally { await app.cleanup(); }
});

test('authentication failure preserves a password draft; success clears it', async () => {
  let fail = true; const calls = [];
  const app = await mount(CloudPage, { cloud: { serverUrl: cloud.serverUrl, configured: true, authenticated: false }, cloudRequest: async (action, payload) => { calls.push({ action, payload }); if (action === 'login' && fail) throw new Error('Incorrect credentials'); return { user: cloud.user }; } });
  try {
    const form = app.container.querySelector('.cp-auth form');
    await input(form.querySelector('[type="email"]'), 'ada@example.com');
    await input(form.querySelector('[type="password"]'), 'local password draft');
    await submit(form);
    assert.match(app.container.querySelector('[role="alert"]').textContent, /Incorrect credentials/);
    assert.equal(form.querySelector('[type="password"]').value, 'local password draft');
    fail = false; await submit(form);
    assert.equal(form.querySelector('[type="password"]').value, '');
    assert.equal(calls.filter(call => call.action === 'login').length, 2);
  } finally { await app.cleanup(); }
});

test('non-admin members do not see permission or invite controls', async () => {
  const app = await mount(CloudPage, { cloud: { ...cloud, teams: [{ id: 't1', name: 'Studio', role: 'member' }] }, cloudRequest: async action => action === 'team.members' ? { members: [{ userId: 'other', name: 'Other member', email: 'other@example.com', role: 'member' }] } : {} });
  try { assert.match(app.container.textContent, /Other member/); assert.equal(app.container.querySelector('.cp-member-controls'), null); assert.equal(app.container.querySelector('.cp-invite'), null); } finally { await app.cleanup(); }
});

test('AI chat does not manufacture responses and retains the message when provider fails', async () => {
  const app = await mount(AgentPage, { cloudRequest: async action => { if (action === 'ai.chat') throw new Error('Provider quota exceeded'); return responses[action] || {}; } });
  try {
    await input(app.container.querySelector('textarea'), 'Where did my time go?');
    const send = findButton(app.container, 'Send message'); assert.equal(send.disabled, true);
    await check(app.container.querySelector('.cp-consent input'), true);
    await submit(app.container.querySelector('form'));
    assert.match(app.container.querySelector('[role="alert"]').textContent, /Provider quota exceeded/);
    assert.equal(app.container.querySelector('textarea').value, 'Where did my time go?');
    assert.equal(app.container.querySelector('.cp-message'), null);
    assert.doesNotMatch(app.container.textContent, /Private window title/);
  } finally { await app.cleanup(); }
});

test('semantic proposals change local tags only after explicit review', async () => {
  const commands = [];
  const app = await mount(AgentPage, { mutate: async command => commands.push(command), cloudRequest: async action => action === 'ai.tag' ? { proposals: [{ entryId: 'e1', clientId: 'c1', projectId: 'p1', confidence: 0.95, method: 'embedding' }], requiresApproval: true } : responses[action] || {} });
  try {
    await click(findButton(app.container, 'Semantic tags'));
    assert.doesNotMatch(app.container.textContent, /Unreviewed private activity|Private window title/);
    await check(app.container.querySelector('.cp-entry-choice input'), true);
    await check(app.container.querySelector('.cp-consent input'), true);
    await click(findButton(app.container, 'Suggest tags for 1 entries'));
    assert.equal(commands.length, 0);
    await click(findButton(app.container, 'Apply reviewed suggestion'));
    assert.deepEqual(commands, [{ type: 'entry.upsert', payload: { id: 'e1', clientId: 'c1', projectId: 'p1' } }]);
  } finally { await app.cleanup(); }
});

test('AI proposal validation drops time/status injection and rejects absent tags', () => {
  const patch = proposalPatch({ entryId: 'e1', description: 'Approved description', start: '1900-01-01', status: 'approved', title: 'Injected title', clientId: 'c1' }, state.entries[0], state);
  assert.deepEqual(patch, { id: 'e1', description: 'Approved description', clientId: 'c1', projectId: '', taskId: '' });
  assert.throws(() => proposalPatch({ entryId: 'e1', projectId: 'missing' }, state.entries[0], state), /not in this workspace/);
});

test('provider token failures preserve input and successful save removes the token', async () => {
  let fail = true; const calls = [];
  const app = await mount(IntegrationsPage, { cloudRequest: async (action, payload) => { calls.push({ action, payload }); if (action === 'integrations.connect' && fail) throw new Error('Encryption key not configured'); return responses[action] || {}; } });
  try {
    const card = app.container.querySelector('.cp-integration');
    await click(findButton(card, 'Connect'));
    await input(card.querySelector('[type="password"]'), 'secret-provider-token');
    await submit(card.querySelector('form'));
    assert.match(app.container.querySelector('[role="alert"]').textContent, /Encryption key not configured/);
    assert.equal(card.querySelector('[type="password"]').value, 'secret-provider-token');
    fail = false; await submit(card.querySelector('form'));
    assert.equal(card.querySelector('[type="password"]'), null);
    await click(findButton(card, 'Connect'));
    assert.equal(card.querySelector('[type="password"]').value, '');
    assert.equal(calls.filter(call => call.action === 'integrations.connect').length, 2);
  } finally { await app.cleanup(); }
});

test('calendar conversion requires timed events and creates only pending manual entries', () => {
  const draft = calendarDraft({ externalId: 'calendar:uid1', title: 'Project meeting', start: '2026-10-17T10:00:00Z', end: '2026-10-17T10:45:00Z', source: 'calendar', status: 'approved' });
  assert.equal(draft.source, 'manual'); assert.equal(draft.status, 'pending'); assert.equal(draft.externalId, 'calendar:uid1');
  assert.throws(() => calendarDraft({ allDay: true, start: '2026-10-17', end: '2026-10-18' }), /Only timed/);
  assert.throws(() => calendarDraft({ start: '2026-10-17T11:00:00Z', end: '2026-10-17T10:00:00Z' }), /Only timed/);
});

test('disconnected AI and integrations pages show setup gates rather than fake activity', () => {
  for (const Page of [AgentPage, IntegrationsPage]) {
    const html = renderToStaticMarkup(React.createElement(Page, { state, cloud: {}, mutate() {}, cloudRequest() {} }));
    assert.match(html, /Connect your workspace first/); assert.doesNotMatch(html, /Connected on server|Configured AI provider/);
  }
});

test('automatic sync is disabled until approved and sends an explicit opt-in', async () => {
  const app = await mount(CloudPage);
  try {
    const controls = app.container.querySelectorAll('.cp-consent input');
    assert.equal(controls[1].checked, false); assert.equal(controls[1].disabled, true);
    assert.equal(app.calls.some(call => call.action === 'sync.auto'), false);
    await check(controls[0], true); assert.equal(controls[1].disabled, false);
    await check(controls[1], true);
    assert.deepEqual(app.calls.find(call => call.action === 'sync.auto').payload, { enabled: true, consent: true });
  } finally { await app.cleanup(); }
});

test('an owner can remove another member but never sees a remove-self control', async () => {
  const calls = [];
  const app = await mount(CloudPage, { cloudRequest: async (action, payload) => { calls.push({ action, payload }); return action === 'team.members' ? { members: [{ userId: 'me', name: 'Ada', email: 'ada@example.com', role: 'owner' }, { userId: 'other', name: 'Other member', email: 'other@example.com', role: 'member' }] } : {}; } });
  try {
    assert.equal(app.container.querySelector('[aria-label="Remove Ada from team"]'), null);
    await click(app.container.querySelector('[aria-label="Remove Other member from team"]'));
    assert.deepEqual(calls.find(call => call.action === 'team.member.remove').payload, { userId: 'other' });
  } finally { await app.cleanup(); }
});

test('enabled routines require consent and save the server interval schema', async () => {
  const calls = [];
  const app = await mount(AgentPage, { cloudRequest: async (action, payload) => { calls.push({ action, payload }); return responses[action] || {}; } });
  try {
    await click(findButton(app.container, 'Routines'));
    await click(findButton(app.container, 'New routine'));
    const form = app.container.querySelector('form');
    await input(form.querySelector('input[type="text"]'), 'Daily review');
    await input(form.querySelector('textarea'), 'Summarize approved project work.');
    await input(form.querySelector('[type="number"]'), '60');
    const checks = form.querySelectorAll('[type="checkbox"]');
    await check(checks[0], true);
    assert.equal(findButton(form, 'Save routine').disabled, true);
    await check(checks[1], true); await submit(form);
    const payload = calls.find(call => call.action === 'ai.routines.save').payload;
    assert.deepEqual(payload, { name: 'Daily review', prompt: 'Summarize approved project work.', intervalMinutes: 60, enabled: true, consent: true });
  } finally { await app.cleanup(); }
});

test('OAuth starts only for configured providers and does not invent a connection', async () => {
  const calls = [];
  const app = await mount(IntegrationsPage, { cloudRequest: async (action, payload) => { calls.push({ action, payload }); if (action === 'integrations.list') return { integrations: [{ provider: 'google', connected: false, oauthConfigured: true }, { provider: 'microsoft', connected: false, oauthConfigured: false }] }; if (action === 'integrations.oauth.start') return { authorizationUrl: 'https://accounts.google.com/o/oauth2/v2/auth?state=temporary' }; return responses[action] || {}; } });
  try {
    assert.equal([...app.container.querySelectorAll('button')].some(button => button.textContent === 'Connect Microsoft'), false);
    await click(findButton(app.container, 'Connect Google'));
    assert.deepEqual(calls.find(call => call.action === 'integrations.oauth.start').payload, { provider: 'google' });
    assert.match(app.container.textContent, /Complete sign-in in your browser/);
    assert.doesNotMatch(app.container.textContent, /Connected on server/);
  } finally { await app.cleanup(); }
});

test('switching an AI client clears incompatible existing project and task links', () => {
  const local = { ...state.entries[0], clientId: 'c1', projectId: 'p1', taskId: 'old-task' };
  const workspace = { ...state, clients: [...state.clients, { id: 'c2', name: 'Another client' }] };
  assert.deepEqual(proposalPatch({ entryId: 'e1', clientId: 'c2' }, local, workspace), { id: 'e1', clientId: 'c2', projectId: '', taskId: '' });
});

test('saved AI reports can be reviewed and edited with optimistic versions', async () => {
  const calls = [];
  const original = { id: 'report1', title: 'Weekly work', text: 'Provider report text', createdAt: '2026-10-17T10:00:00.000Z', version: 3 };
  const app = await mount(AgentPage, { cloudRequest: async (action, payload) => { calls.push({ action, payload }); if (action === 'ai.reports.list') return { reports: [original] }; if (action === 'ai.reports.save') return { report: { ...original, ...payload, version: 4 } }; return responses[action] || {}; } });
  try {
    await click(findButton(app.container, 'Reports'));
    assert.match(app.container.textContent, /Provider report text/);
    await click(findButton(app.container, 'Edit report'));
    const form = app.container.querySelector('.cp-editor');
    await input(form.querySelector('input'), 'Reviewed weekly work');
    await input(form.querySelector('textarea'), 'Human reviewed report text');
    await submit(form);
    assert.deepEqual(calls.find(call => call.action === 'ai.reports.save').payload, { id: 'report1', title: 'Reviewed weekly work', text: 'Human reviewed report text', baseVersion: 3 });
    assert.match(app.container.textContent, /Human reviewed report text/);
    assert.equal(app.container.querySelector('.cp-editor'), null);
  } finally { await app.cleanup(); }
});

test('OAuth calendar pull requires sharing consent and reviewed events become pending entries', async () => {
  const calls = [], commands = [];
  const app = await mount(IntegrationsPage, { mutate: async command => commands.push(command), cloudRequest: async (action, payload) => { calls.push({ action, payload }); if (action === 'integrations.list') return { integrations: [{ provider: 'google', connected: true, oauthConfigured: true }] }; if (action === 'integrations.sync') return { imported: 1, events: [], proposals: [{ externalId: 'google-event-1', title: 'Real calendar meeting', start: '2026-10-17T10:00:00Z', end: '2026-10-17T10:30:00Z' }], warnings: [] }; return responses[action] || {}; } });
  try {
    const row = [...app.container.querySelectorAll('.cp-oauth-status')].find(element => element.textContent.includes('Google Calendar'));
    const button = findButton(row, 'Import now'); assert.equal(button.disabled, true);
    await check(row.querySelector('[type="checkbox"]'), true);
    await click(button);
    assert.deepEqual(calls.find(call => call.action === 'integrations.sync').payload, { provider: 'google', consent: true });
    assert.equal(commands.length, 0, 'Calendar pull must not create local time without review');
    await click(findButton(app.container, 'Create pending entry'));
    assert.equal(commands[0].type, 'entry.upsert'); assert.equal(commands[0].payload.status, 'pending'); assert.equal(commands[0].payload.source, 'manual');
  } finally { await app.cleanup(); }
});
