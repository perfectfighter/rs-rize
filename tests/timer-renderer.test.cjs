const test = require('node:test');
const assert = require('node:assert/strict');
const path = require('node:path');
const Module = require('node:module');
const React = require('react');
const { renderToStaticMarkup } = require('react-dom/server');
const { buildSync } = require('esbuild');

const source = buildSync({
  entryPoints: [path.join(__dirname, '../src/TimerPage.jsx')],
  bundle: true,
  write: false,
  platform: 'node',
  format: 'cjs',
  external: ['react'],
  loader: { '.css': 'empty' },
}).outputFiles[0].text;
const compiled = new Module(path.join(__dirname, 'timer-renderer-compiled.cjs'), module);
compiled.filename = path.join(__dirname, 'timer-renderer-compiled.cjs');
compiled.paths = module.paths;
compiled._compile(source, compiled.filename);
const { default: TimerPage, getTimerDraft, timerElapsed, sessionDayElapsed } = compiled.exports;
const MINUTE = 60_000;
const state = {
  clients: [{ id: 'c1', name: 'North Studio' }, { id: 'c2', name: 'South Studio' }],
  projects: [{ id: 'p1', name: 'Website launch', clientId: 'c1' }, { id: 'p2', name: 'Research', clientId: 'c2' }],
  tasks: [{ id: 't1', name: 'Draft outline', projectId: 'p1', clientId: 'c1' }],
  sessions: [],
  timer: null,
};
test.afterEach(() => { delete globalThis.localStorage; });

test('default draft is a 45 minute focus session and corrupt storage is recoverable', () => {
  globalThis.localStorage = { getItem() { return '{invalid'; } };
  assert.deepEqual(getTimerDraft(state), { kind: 'focus', goal: '', targetMs: 45 * MINUTE, clientId: '', projectId: '', taskId: '' });
});

test('persisted timer drafts clamp targets to 24h and discard incompatible links', () => {
  globalThis.localStorage = { getItem() { return JSON.stringify({ kind: 'meeting', targetMs: 2000 * MINUTE, goal: 'Prepare\u0000 notes', clientId: 'c1', projectId: 'p2', taskId: 'missing' }); } };
  assert.deepEqual(getTimerDraft(state), { kind: 'meeting', goal: 'Prepare notes', targetMs: 1440 * MINUTE, clientId: 'c1', projectId: '', taskId: '' });
});

test('kind-specific defaults and valid linked tags survive draft reload', () => {
  globalThis.localStorage = { getItem() { return JSON.stringify({ kind: 'break', clientId: 'c1', projectId: 'p1', taskId: 't1' }); } };
  assert.equal(getTimerDraft(state).targetMs, 5 * MINUTE);
  assert.equal(getTimerDraft(state).taskId, 't1');
  globalThis.localStorage = { getItem() { return JSON.stringify({ kind: 'meeting' }); } };
  assert.equal(getTimerDraft(state).targetMs, 30 * MINUTE);
});

test('elapsed time counts actual resumed interval and never includes paused time', () => {
  const running = { status: 'running', elapsedMs: 10 * MINUTE, resumedAt: '2026-10-17T10:30:00.000Z' };
  assert.equal(timerElapsed(running, Date.parse('2026-10-17T10:35:00.000Z')), 15 * MINUTE);
  assert.equal(timerElapsed({ ...running, status: 'paused' }, Date.parse('2026-10-17T11:35:00.000Z')), 10 * MINUTE);
  assert.equal(timerElapsed(running, Date.parse('2026-10-17T10:29:59.000Z')), 10 * MINUTE);
});

test('today totals clip actual intervals at local midnight and exclude pause gaps', () => {
  const midnight = new Date(2026, 9, 17, 0, 0, 0).getTime();
  const session = { elapsedMs: 30 * MINUTE, segments: [
    { start: new Date(midnight - 10 * MINUTE).toISOString(), end: new Date(midnight + 5 * MINUTE).toISOString() },
    { start: new Date(midnight + 20 * MINUTE).toISOString(), end: new Date(midnight + 35 * MINUTE).toISOString() },
  ] };
  assert.equal(sessionDayElapsed(session, midnight + 60 * MINUTE), 20 * MINUTE);
  assert.equal(sessionDayElapsed(session, midnight - 60 * MINUTE), 10 * MINUTE);
});

test('active session renderer uses stored metadata and shows the accurate paused countdown', () => {
  const timer = { id: 'timer1', kind: 'focus', status: 'paused', goal: 'Ship the desktop timer', targetMs: 45 * MINUTE, elapsedMs: 10 * MINUTE, resumedAt: null, clientId: 'c1', projectId: 'p1', taskId: 't1' };
  const html = renderToStaticMarkup(React.createElement(TimerPage, { state: { ...state, timer }, mutate() {}, now: Date.parse('2026-10-17T12:00:00Z') }));
  assert.match(html, /35:00/);
  assert.match(html, /Resume session/);
  assert.match(html, /paused time is excluded/);
  assert.match(html, /Ship the desktop timer/);
  assert.match(html, /value="p1" selected=""/);
  assert.match(html, /value="t1" selected=""/);
  assert.doesNotMatch(html, /Start focus session/);
});

test('history renders actual saved sessions and empty state has no invented sessions', () => {
  const empty = renderToStaticMarkup(React.createElement(TimerPage, { state, mutate() {}, now: Date.parse('2026-10-17T12:00:00Z') }));
  assert.match(empty, /Your first session starts here/);
  const history = renderToStaticMarkup(React.createElement(TimerPage, { state: { ...state, sessions: [{ id: 's1', kind: 'meeting', goal: 'Client review', startedAt: '2026-10-17T11:00:00Z', endedAt: '2026-10-17T11:30:00Z', elapsedMs: 25 * MINUTE, projectId: 'p1' }] }, mutate() {}, now: Date.parse('2026-10-17T12:00:00Z') }));
  assert.match(history, /Client review/);
  assert.match(history, /25:00/);
  assert.match(history, /Website launch/);
  assert.doesNotMatch(history, /Your first session starts here/);
});
