const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const adapterModule = import('../src/native-bridge-core.js');
const settle = () => new Promise(resolve => setImmediate(resolve));
function deferred() { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; }

test('existing renderer methods map to exact Rust commands and preserve envelopes', async () => {
  const { createNativeBridge } = await adapterModule;
  const calls = [];
  const envelope = { cloud: { authenticated: true }, result: { cursor: 5 }, workspace: { schemaVersion: 1 } };
  const bridge = createNativeBridge({ invoke: async (...args) => { calls.push(args); return envelope; }, listen: async () => () => {} });
  const command = { type: 'entry.upsert', payload: { id: 'e', description: 'Reviewed' } };
  const query = { startDate: '2026-10-01', endDate: '2026-10-31' };
  const payload = { consent: true, resolutions: [] };
  const actions = [
    [() => bridge.getState(), ['rize_get_state', undefined]],
    [() => bridge.getStatus(), ['rize_get_status', undefined]],
    [() => bridge.mutate(command), ['rize_mutate', { command }]],
    [() => bridge.windowAction('maximize'), ['rize_window', { action: 'maximize' }]],
    [() => bridge.exportData('csv'), ['rize_export', { format: 'csv' }]],
    [() => bridge.importData(), ['rize_import', undefined]],
    [() => bridge.cloud('sync', payload), ['rize_cloud', { action: 'sync', payload }]],
    [() => bridge.cloud('status'), ['rize_cloud', { action: 'status', payload: {} }]],
    [() => bridge.businessReport(query), ['rize_business_report', { query }]],
    [() => bridge.businessReport(), ['rize_business_report', { query: {} }]],
    [() => bridge.invoicePdf({ id: 'invoice' }), ['rize_invoice_pdf', { payload: { id: 'invoice' } }]],
    [() => bridge.updates('check'), ['rize_updates', { action: 'check' }]],
  ];
  for (const [run, expected] of actions) { assert.equal(await run(), envelope); assert.deepEqual(calls.at(-1), expected); }
  assert.equal(calls.length, actions.length);
  assert.ok(Object.isFrozen(bridge));
  assert.equal(bridge.invoke, undefined);
  assert.equal(bridge.listen, undefined);
});

test('Rust string and structured failures become useful Error objects without losing existing errors', async () => {
  const { createNativeBridge } = await adapterModule;
  let failure = 'Select a team before synchronizing.';
  const bridge = createNativeBridge({ invoke: () => Promise.reject(failure), listen: async () => () => {} });
  await assert.rejects(bridge.cloud('sync'), error => error instanceof Error && error.message === failure);
  failure = { message: 'Workspace was preserved.', code: 'DISK_FULL' };
  await assert.rejects(bridge.mutate({}), error => error.message === failure.message && error.code === 'DISK_FULL');
  failure = new TypeError('Invalid draft');
  await assert.rejects(bridge.getState(), error => error === failure);
  failure = { secret: 'Do not stringify unknown failure objects' };
  await assert.rejects(bridge.getStatus(), error => error.message === 'The desktop operation failed. Please retry.' && !error.message.includes('secret'));
});

test('synchronous adapter errors use the same promise rejection contract', async () => {
  const { createNativeBridge } = await adapterModule;
  const bridge = createNativeBridge({ invoke: () => { throw 'Cannot open workspace'; }, listen: async () => () => {} });
  await assert.rejects(bridge.getState(), /Cannot open workspace/);
});

test('state and status events deliver payload only, unsubscribe exactly once, and reject invalid callbacks', async () => {
  const { createNativeBridge } = await adapterModule;
  const listeners = new Map(), received = [];
  let removed = 0;
  const bridge = createNativeBridge({ invoke: async () => {}, listen: async (name, callback) => { listeners.set(name, callback); return () => removed++; } });
  assert.throws(() => bridge.onState(null), /callback is required/);
  const offState = bridge.onState(value => received.push(['state', value]));
  const offStatus = bridge.onStatus(value => received.push(['status', value]));
  await settle();
  const state = { entries: [] }, status = { tracking: { enabled: false } };
  listeners.get('rize:state')({ payload: state, id: 18, event: 'rize:state' });
  listeners.get('rize:status')({ payload: status, id: 19, event: 'rize:status' });
  assert.deepEqual(received, [['state', state], ['status', status]]);
  offState(); offState(); offStatus();
  assert.equal(removed, 2);
  listeners.get('rize:state')({ payload: { leaked: true } });
  assert.equal(received.length, 2);
});

test('React cleanup before asynchronous registration finishes removes the late native listener', async () => {
  const { createNativeBridge } = await adapterModule;
  const pending = deferred(); let callback, removed = 0, received = 0;
  const bridge = createNativeBridge({ invoke: async () => {}, listen: (_name, handler) => { callback = handler; return pending.promise; } });
  const off = bridge.onState(() => received++);
  off(); off();
  callback({ payload: { stale: true } });
  pending.resolve(() => removed++);
  await settle();
  off();
  assert.equal(removed, 1);
  assert.equal(received, 0);
});

test('listener registration and cleanup failures are reported without unhandled promises', async () => {
  const { createNativeBridge } = await adapterModule;
  const failures = [];
  const bridge = createNativeBridge({ invoke: async () => {}, listen: () => Promise.reject('Native events unavailable'), onListenerError: error => failures.push(error) });
  bridge.onState(() => {})();
  await settle();
  assert.equal(failures[0].message, 'Native events unavailable');
  const cleanupBridge = createNativeBridge({ invoke: async () => {}, listen: async () => () => { throw new Error('Cleanup failed'); }, onListenerError: error => failures.push(error) });
  const off = cleanupBridge.onStatus(() => {});
  await settle(); off(); off();
  assert.equal(failures.length, 2);
  assert.equal(failures[1].message, 'Cleanup failed');
  const asyncCleanup = createNativeBridge({ invoke: async () => {}, listen: async () => async () => { throw 'Asynchronous cleanup failed'; }, onListenerError: error => failures.push(error) });
  const offAsync = asyncCleanup.onState(() => {});
  await settle(); offAsync(); await settle();
  assert.equal(failures.length, 3);
  assert.equal(failures[2].message, 'Asynchronous cleanup failed');
});

test('browser preview exposes no native bridge and Tauri installs a frozen narrow capability', async () => {
  const { installNativeBridge } = await adapterModule;
  const preview = {};
  assert.equal(installNativeBridge({ target: preview, isNative: () => false }), null);
  assert.equal(Object.hasOwn(preview, 'rize'), false);
  const target = {};
  const bridge = installNativeBridge({ target, isNative: () => true, invoke: async () => 'native', listen: async () => () => {} });
  assert.equal(target.rize, bridge);
  assert.equal(await bridge.getState(), 'native');
  const descriptor = Object.getOwnPropertyDescriptor(target, 'rize');
  assert.equal(descriptor.writable, false);
  assert.equal(descriptor.configurable, false);
});

test('native bridge initializes before React mounts and window controls are excluded from drag regions', () => {
  const main = fs.readFileSync(path.join(__dirname, '../src/main.jsx'), 'utf8');
  const app = fs.readFileSync(path.join(__dirname, '../src/App.jsx'), 'utf8');
  assert.ok(main.indexOf("import './native-bridge.js'") < main.indexOf('createRoot(document'));
  assert.match(app, /<header className="titlebar" data-tauri-drag-region>/);
  assert.match(app, /<span className="title-caption" data-tauri-drag-region>/);
  assert.doesNotMatch(app, /(?:className="window-controls"|aria-label="Minimize")[^>]*data-tauri-drag-region/);
});
