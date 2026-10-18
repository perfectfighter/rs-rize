const test = require('node:test');
const assert = require('node:assert/strict');
const path = require('node:path');
const { parseHTML } = require('linkedom');
const { window } = parseHTML('<!doctype html><html><body></body></html>');
window.document.oninput = null;
for (const name of ['window', 'document', 'HTMLElement', 'HTMLInputElement', 'HTMLTextAreaElement', 'HTMLSelectElement', 'Event', 'MouseEvent', 'Node']) if (window[name]) Object.defineProperty(globalThis, name, { value: window[name], configurable: true, writable: true });
Object.defineProperty(globalThis, 'navigator', { value: { userAgent: 'Linkedom app navigation test' }, configurable: true });
globalThis.IS_REACT_ACT_ENVIRONMENT = true;

// Linkedom has no media decoder or focus manager. Model only those browser
// primitives, leaving the real React navigation and editor components intact.
let focused = document.body;
Object.defineProperty(document, 'activeElement', { get: () => focused, configurable: true });
window.HTMLElement.prototype.focus = function () { focused = this; };
const makeElement = document.createElement.bind(document);
document.createElement = (tag, ...args) => {
  const element = makeElement(tag, ...args);
  if (tag.toLowerCase() === 'audio') { element.pause = () => {}; element.play = async () => {}; }
  return element;
};
const React = require('react');
const { act } = React;
const { createRoot } = require('react-dom/client');
function compile(file) {
  const result = require('esbuild').buildSync({ entryPoints: [path.join(__dirname, '../src', file)], bundle: true, write: false, platform: 'node', format: 'cjs', external: ['react', 'react-dom', 'react-feather'], loader: { '.css': 'empty', '.png': 'dataurl', '.webp': 'dataurl' } });
  const scope = { exports: {} };
  new Function('require', 'module', 'exports', result.outputFiles[0].text)(require, scope, scope.exports);
  return scope.exports;
}
const App = compile('App.jsx').default;
const { WorkPage } = compile('WorkPages.jsx');
const click = async node => { assert.ok(node, 'Expected a reachable control'); await act(async () => node.dispatchEvent(new window.Event('click', { bubbles: true }))); };
const button = (container, name) => [...container.querySelectorAll('button')].find(node => node.textContent.trim() === name);
async function mount(Component, props) {
  const container = document.createElement('div'); document.body.append(container);
  const root = createRoot(container);
  await act(async () => root.render(React.createElement(Component, props)));
  return { container, cleanup: async () => { await act(async () => root.unmount()); container.remove(); } };
}
function keyboard(key, shiftKey = false) {
  const event = new window.Event('keydown', { bubbles: true, cancelable: true });
  Object.defineProperties(event, { key: { value: key }, shiftKey: { value: shiftKey } });
  return event;
}

test('Calendar opens by default and every grouped navigation destination remains reachable', async () => {
  const app = await mount(App);
  try {
    assert.ok(app.container.querySelector('.cal-page'));
    assert.equal(app.container.querySelector('[aria-current="page"] > span').textContent, 'Calendar');
    assert.deepEqual([...app.container.querySelectorAll('.nav-group > h2')].map(node => node.textContent), ['Home', 'Track', 'Insights', 'Work', 'Admin']);
    const expected = ['Agent', 'Calendar', 'My Timesheet', 'Timer', 'Activity', 'Reports', 'Profitability', 'Tasks', 'Projects', 'Clients', 'Teams', 'Resourcing', 'Invoices'];
    const links = [...app.container.querySelectorAll('nav[aria-label="Main navigation"] .nav-item')];
    assert.deepEqual(links.map(node => node.querySelector('span').textContent), expected);
    for (const link of links) {
      await click(link);
      assert.equal(link.getAttribute('aria-current'), 'page');
      assert.ok(app.container.querySelector('.page-content h1'), `Expected screen title for ${link.textContent}`);
      assert.ok(app.container.querySelector('.session-footer'));
    }
    await click(app.container.querySelector('[aria-label="Collapse navigation"]'));
    assert.ok(app.container.querySelector('.desktop-app.is-collapsed'));
    await click(app.container.querySelector('[aria-label="Expand navigation"]'));
    assert.equal(app.container.querySelector('.desktop-app.is-collapsed'), null);
  } finally { await app.cleanup(); }
});

test('workspace menu and footer retain secondary destinations and mounted music state', async () => {
  const app = await mount(App);
  try {
    const music = app.container.querySelector('.music-page');
    await click(app.container.querySelector('.footer-music'));
    const pink = [...music.querySelectorAll('.music-sound')].find(node => node.querySelector('strong').textContent === 'Pink noise');
    await click(pink);
    for (const [name, selector] of [['Overview', '.home-page'], ['Integrations', '.cp-page'], ['Music', '.music-page'], ['Updates', '.work-page'], ['Settings', '.wp-settings']]) {
      const menu = app.container.querySelector('.workspace-menu'); menu.open = true;
      assert.ok(menu.querySelector('summary[title="Workspace menu"]'));
      await click(button(menu, name));
      assert.equal(menu.open, false);
      assert.ok(app.container.querySelector(selector), `Expected ${name} destination`);
      assert.equal(app.container.querySelector('.music-page'), music);
      assert.ok(pink.classList.contains('selected'));
    }
    await click(app.container.querySelector('.footer-agent'));
    assert.ok(app.container.querySelector('.cp-agent'));
    await click(app.container.querySelector('[aria-label="Workspace overview"]'));
    assert.ok(app.container.querySelector('.home-page'));
    await click(app.container.querySelector('.footer-start'));
    assert.match(app.container.querySelector('.error-toast').textContent, /read-only/);
    assert.equal(app.container.querySelector('.footer-tracking').disabled, true);
  } finally { await app.cleanup(); }
});

test('entity modal preserves failed drafts and restores focus on Escape and close', async () => {
  const state = { clients: [{ id: 'c', name: 'Client', hourlyRate: 80 }], projects: [{ id: 'p', name: 'Project', clientId: 'c', keywords: ['design'] }], tasks: [], entries: [] };
  const calls = [];
  const app = await mount(WorkPage, { kind: 'projects', state, mutate: async command => { calls.push(command); throw new Error('Disk unavailable'); } });
  try {
    const opener = app.container.querySelector('[aria-label="Edit Project"]'); opener.focus();
    await click(opener);
    const dialog = app.container.querySelector('form[role="dialog"][aria-modal="true"]');
    assert.equal(dialog.getAttribute('aria-labelledby'), 'wp-editor-title');
    const name = dialog.querySelector('input');
    assert.equal(document.activeElement, name);
    // Browser input.type defaults to text; Linkedom leaves it unset.
    name.type = 'text';
    Object.getOwnPropertyDescriptor(Object.getPrototypeOf(name), 'value').set.call(name, 'Reviewed project');
    await act(async () => name.dispatchEvent(new window.Event('input', { bubbles: true })));
    await act(async () => dialog.dispatchEvent(new window.Event('submit', { bubbles: true, cancelable: true })));
    assert.deepEqual(calls, [{ type: 'project.upsert', payload: { name: 'Reviewed project', id: 'p', clientId: 'c', keywords: ['design'] } }]);
    assert.match(dialog.querySelector('[role="alert"]').textContent, /Disk unavailable/);
    assert.equal(name.value, 'Reviewed project');
    const controls = [...dialog.querySelectorAll('button:not([disabled]),input:not([disabled]),select:not([disabled]),textarea:not([disabled])')];
    controls.at(-1).focus();
    await act(async () => document.dispatchEvent(keyboard('Tab')));
    assert.equal(document.activeElement, controls[0]);
    await act(async () => document.dispatchEvent(keyboard('Escape')));
    assert.equal(app.container.querySelector('[role="dialog"]'), null);
    assert.equal(document.activeElement, opener);
    await click(opener);
    await click(app.container.querySelector('[aria-label="Close editor"]'));
    assert.equal(app.container.querySelector('[role="dialog"]'), null);
    assert.equal(document.activeElement, opener);
  } finally { await app.cleanup(); }
});
