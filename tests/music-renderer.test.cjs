'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const path = require('node:path');
const Module = require('node:module');
const React = require('react');
const { renderToStaticMarkup } = require('react-dom/server');
const { buildSync } = require('esbuild');
const source = buildSync({ entryPoints: [path.join(__dirname, '../src/MusicPlayer.jsx')], bundle: true, write: false, platform: 'node', format: 'cjs', external: ['react'], loader: { '.css': 'empty' } }).outputFiles[0].text;
const compiled = new Module(path.join(__dirname, 'music-renderer-compiled.cjs'), module); compiled.filename = path.join(__dirname, 'music-renderer-compiled.cjs'); compiled.paths = module.paths; compiled._compile(source, compiled.filename);
const { default: MusicPage, createFocusAudio, acceptsAudioFile } = compiled.exports;
function fakeContext() {
  const nodes = []; const values = [];
  function node() { const result = { stopped: false, disconnected: false, connect() {}, start() { this.started = true; }, stop() { this.stopped = true; }, disconnect() { this.disconnected = true; } }; nodes.push(result); return result; }
  class Context { constructor() { this.state = 'running'; this.sampleRate = 1000; this.currentTime = 0; this.destination = {}; Context.instances.push(this); } resume() { return Promise.resolve(); } close() { this.state = 'closed'; return Promise.resolve(); } createGain() { return { ...node(), gain: { setValueAtTime(...a) { values.push(a); }, linearRampToValueAtTime(...a) { values.push(a); }, setTargetAtTime(...a) { values.push(a); } } }; } createOscillator() { return Object.assign(node(), { frequency: { value: 0 } }); } createBuffer(channels, length) { const data = new Float32Array(length); this.data = data; return { getChannelData: () => data }; } createBufferSource() { return node(); } }
  Context.instances = []; return { Context, nodes, values };
}
test('focus audio is paused by default with real local options and file controls', () => {
  const html = renderToStaticMarkup(React.createElement(MusicPage));
  assert.match(html, /Brown noise/); assert.match(html, /Pink noise/); assert.match(html, /Ambient tones/); assert.match(html, /aria-label="Play audio"/); assert.match(html, /Choose audio/); assert.match(html, /never copied/); assert.doesNotMatch(html, /(?:src|href)="https?:\/\//);
});
test('local audio accepts audio formats and rejects empty and nonaudio files', () => {
  assert.equal(acceptsAudioFile({ name: 'focus.mp3', type: '', size: 10 }), true);
  assert.equal(acceptsAudioFile({ name: 'focus.any', type: 'audio/ogg', size: 10 }), true);
  assert.equal(acceptsAudioFile({ name: 'focus.mp3', type: 'text/html', size: 10 }), false);
  assert.equal(acceptsAudioFile({ name: 'focus.mp3', type: 'audio/mpeg', size: 0 }), false);
});
test('noise buffers contain actual finite samples and loop until explicit stop', async () => {
  const { Context, nodes, values } = fakeContext(); let i = 0;
  const audio = createFocusAudio({ AudioContext: Context, random: () => ((i++ * 0.61803398875) % 1) });
  await audio.start('brown', 0.5);
  assert.equal(nodes.at(-1).loop, true);
  assert.ok(Context.instances[0].data.some(x => x !== 0));
  assert.ok(Context.instances[0].data.every(Number.isFinite));
  audio.setVolume(2); assert.equal(values.at(-1)[0], 0.3);
  await audio.start('pink', 0.2);
  assert.equal(Context.instances[0].state, 'closed'); assert.equal(nodes[1].stopped, true);
  assert.ok(Context.instances[1].data.every(Number.isFinite));
  audio.stop(); assert.equal(Context.instances[1].state, 'closed'); assert.equal(nodes.at(-1).stopped, true);
});
test('ambient tones start real oscillators and unavailable audio returns an actionable error', async () => {
  const { Context, nodes } = fakeContext(); const audio = createFocusAudio({ AudioContext: Context });
  await audio.start('tones', 0.25);
  assert.equal(nodes.filter(n => n.started).length, 3); audio.stop();
  await assert.rejects(createFocusAudio({ AudioContext: null }).start(), /Web Audio is unavailable/);
  await assert.rejects(audio.start('remote-track'), /supported focus sound/);
});
