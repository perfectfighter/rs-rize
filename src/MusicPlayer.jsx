import React, { useEffect, useRef, useState } from 'react';
import { Headphones, Music, Pause, Play, Upload, Volume2, X } from 'react-feather';
import './MusicPlayer.css';

export const FOCUS_SOUNDS = [
  { id: 'brown', name: 'Brown noise', kind: 'A softer, low frequency wash', detail: 'A steady background for deep work.', color: '#b89b79' },
  { id: 'pink', name: 'Pink noise', kind: 'Balanced, textured noise', detail: 'A gentle layer across the spectrum.', color: '#c38fae' },
  { id: 'tones', name: 'Ambient tones', kind: 'A quiet, sustained chord', detail: 'Three sine waves with a slow fade in.', color: '#9b8fc9' },
];

// Created in the renderer on an explicit play gesture; generated locally without
// remote tracks, telemetry, microphone access, or third-party audio services.
export function createFocusAudio({ AudioContext: Context = globalThis.AudioContext || globalThis.webkitAudioContext, random = Math.random } = {}) {
  let context = null, gain = null, nodes = [], generation = 0, requestedLevel = 0.25;
  function volume(value) { const level = Number(value); return Number.isFinite(level) ? Math.max(0, Math.min(1, level)) : 0.25; }
  function setVolume(value) { requestedLevel = volume(value); if (gain && context) gain.gain.setTargetAtTime(requestedLevel * 0.3, context.currentTime, 0.04); }
  function stop() {
    generation += 1;
    for (const node of nodes) { try { node.stop(); } catch { /* already stopped */ } try { node.disconnect(); } catch { /* already disconnected */ } }
    nodes = [];
    try { gain?.disconnect(); } catch { /* already disconnected */ }
    gain = null;
    const old = context; context = null;
    if (old && old.state !== 'closed') Promise.resolve(old.close()).catch(() => {});
  }
  async function start(sound = 'brown', level = 0.25) {
    if (!['brown', 'pink', 'tones'].includes(sound)) throw new Error('Choose a supported focus sound.');
    if (!Context) throw new Error('Web Audio is unavailable in this app environment.');
    stop(); requestedLevel = volume(level);
    const token = generation;
    const next = new Context(); context = next;
    try {
      await next.resume();
      if (generation !== token || context !== next) return false;
      gain = next.createGain(); gain.gain.setValueAtTime(0, next.currentTime); gain.gain.linearRampToValueAtTime(requestedLevel * 0.3, next.currentTime + 0.2); gain.connect(next.destination);
      if (sound === 'tones') {
        for (const frequency of [174.61, 261.63, 349.23]) { const oscillator = next.createOscillator(); oscillator.type = 'sine'; oscillator.frequency.value = frequency; oscillator.connect(gain); oscillator.start(); nodes.push(oscillator); }
      } else {
        const length = Math.round(next.sampleRate * 8), buffer = next.createBuffer(1, length, next.sampleRate), data = buffer.getChannelData(0);
        let previous = 0, b0 = 0, b1 = 0, b2 = 0, b3 = 0, b4 = 0, b5 = 0, b6 = 0;
        for (let i = 0; i < length; i += 1) {
          const white = random() * 2 - 1;
          if (sound === 'brown') { previous = (previous + 0.02 * white) / 1.02; data[i] = previous * 3.5; }
          else { b0 = 0.99886 * b0 + white * 0.0555179; b1 = 0.99332 * b1 + white * 0.0750759; b2 = 0.969 * b2 + white * 0.153852; b3 = 0.8665 * b3 + white * 0.3104856; b4 = 0.55 * b4 + white * 0.5329522; b5 = -0.7616 * b5 - white * 0.016898; data[i] = (b0 + b1 + b2 + b3 + b4 + b5 + b6 + white * 0.5362) * 0.11; b6 = white * 0.115926; }
        }
        // Remove a click at the loop seam without adding a second audio source.
        const blend = Math.min(256, Math.floor(length / 10));
        for (let i = 0; i < blend; i += 1) { const mix = i / blend; data[length - blend + i] = data[length - blend + i] * (1 - mix) + data[i] * mix; }
        const source = next.createBufferSource(); source.buffer = buffer; source.loop = true; source.connect(gain); source.start(); nodes.push(source);
      }
      return true;
    } catch (error) { if (context === next) stop(); throw error; }
  }
  return { start, stop, setVolume };
}

export function acceptsAudioFile(file) {
  if (!file || file.size === 0) return false;
  if (file.type) return /^audio\//i.test(file.type);
  return /\.(mp3|wav|ogg|oga|m4a|aac|flac|opus|webm)$/i.test(file.name || '');
}
export default function MusicPage() {
  const [sound, setSound] = useState('brown'), [level, setLevel] = useState(0.25), [playing, setPlaying] = useState(false), [busy, setBusy] = useState(false), [error, setError] = useState(''), [file, setFile] = useState(null);
  const engine = useRef(null), audio = useRef(null), input = useRef(null), fileUrl = useRef(null), generation = useRef(0);
  const halt = () => { generation.current += 1; engine.current?.stop(); if (audio.current) audio.current.pause(); setPlaying(false); setBusy(false); };
  useEffect(() => () => { generation.current += 1; engine.current?.stop(); audio.current?.pause(); if (fileUrl.current) URL.revokeObjectURL(fileUrl.current); }, []);
  const choose = id => { halt(); setSound(id); setError(''); };
  const play = async () => {
    if (playing) { halt(); return; }
    setBusy(true); setError(''); const token = ++generation.current;
    try {
      if (sound === 'file') {
        if (!file || !audio.current) throw new Error('Choose an audio file first.');
        audio.current.volume = level; await audio.current.play();
      } else {
        engine.current ||= createFocusAudio();
        const started = await engine.current.start(sound, level);
        if (!started) return;
      }
      if (generation.current === token) setPlaying(true);
      else { engine.current?.stop(); audio.current?.pause(); }
    } catch (err) { if (generation.current === token) { setError(err.message || 'This audio could not be played. Try another format.'); setPlaying(false); } }
    finally { if (generation.current === token) setBusy(false); }
  };
  const upload = event => {
    const picked = event.target.files?.[0]; event.target.value = '';
    if (!picked) return;
    if (!acceptsAudioFile(picked)) { setError('Choose a nonempty audio file, such as MP3, WAV, M4A, or OGG.'); return; }
    halt();
    if (fileUrl.current) URL.revokeObjectURL(fileUrl.current);
    const url = URL.createObjectURL(picked); fileUrl.current = url;
    setFile({ name: picked.name, url }); setSound('file'); setError('');
  };
  const removeFile = () => { halt(); if (fileUrl.current) URL.revokeObjectURL(fileUrl.current); fileUrl.current = null; setFile(null); setSound('brown'); };
  const selected = FOCUS_SOUNDS.find(item => item.id === sound);
  return <div className="music-page">
    <header className="music-header"><Headphones size={19}/><h1>Focus audio</h1><span>Set the tone for your next session</span></header>
    <main><div className="music-intro"><span className="music-eyebrow">YOUR SPACE TO FOCUS</span><h2>A little less distraction.</h2><p>Locally generated sound, or music from your own collection. Nothing is streamed or uploaded.</p></div>
      <div className="music-sound-grid">{FOCUS_SOUNDS.map(item => <button key={item.id} onClick={() => choose(item.id)} className={`music-sound ${sound === item.id ? 'selected' : ''}`} aria-pressed={sound === item.id} style={{ '--sound-color': item.color }}><span className="music-sound-art"><Headphones size={37}/><i/><i/><i/></span><strong>{item.name}</strong><span>{item.kind}</span><small>{item.detail}</small></button>)}</div>
      <section className={`music-upload ${sound === 'file' ? 'selected' : ''}`}><Music size={24}/><div><h3>{file ? file.name : 'Your own soundtrack'}</h3><p>{file ? 'Local file · repeats while playing · stays on this computer' : 'Choose a local audio file. Your file is never copied to the workspace or uploaded.'}</p></div>{file && <button onClick={() => choose('file')}>Select file</button>}<button onClick={() => input.current?.click()}><Upload size={14}/>{file ? 'Replace file' : 'Choose audio'}</button>{file && <button aria-label="Remove audio file" onClick={removeFile}><X size={15}/></button>}<input ref={input} type="file" accept="audio/*,.mp3,.wav,.ogg,.m4a,.aac,.flac,.opus" onChange={upload} hidden/></section>
      <section className="music-player" aria-label="Audio playback controls"><button className="music-play" aria-label={playing ? 'Pause audio' : 'Play audio'} disabled={busy || (sound === 'file' && !file)} onClick={play}>{playing ? <Pause size={22}/> : <Play size={22}/>}</button><div className="music-now"><strong>{selected?.name || file?.name || 'Choose audio'}</strong><span aria-live="polite">{busy ? 'Starting audio…' : playing ? 'Playing · continuous loop' : 'Paused · press play when ready'}</span></div><label className="music-volume"><Volume2 size={17}/><span className="music-sr-only">Volume</span><input aria-label="Volume" type="range" min="0" max="1" step="0.01" value={level} onChange={event => { const value = Number(event.target.value); setLevel(value); engine.current?.setVolume(value); if (audio.current) audio.current.volume = value; }}/><output>{Math.round(level * 100)}%</output></label></section>
      {error && <p className="music-error" role="alert">{error}</p>}<p className="music-note">Audio continues while you navigate. Pause playback here when finished, and choose a comfortable listening level. Generated sounds carry no medical or productivity claims.</p>
      <audio ref={audio} src={file?.url} loop preload="metadata" onError={() => { halt(); setError('This file format could not be decoded. Choose an audio format supported by this Windows app.'); }}/>
    </main>
  </div>;
}
