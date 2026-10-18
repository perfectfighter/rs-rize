import React, { useEffect, useId, useMemo, useState } from 'react';
import './TimerPage.css';

const STORAGE_KEY = 'rize-local.timer-draft.v1';
const MINUTE = 60_000;
const MAX_TARGET = 1_440 * MINUTE;
const KINDS = {
  focus: { label: 'Focus', minutes: 45, description: 'Make room for your most important work.' },
  meeting: { label: 'Meeting', minutes: 30, description: 'Give the conversation your full attention.' },
  break: { label: 'Break', minutes: 5, description: 'Step away, recharge, and come back refreshed.' },
};

function linkedTags(input, state = {}) {
  const clients = Array.isArray(state.clients) ? state.clients : [];
  const projects = Array.isArray(state.projects) ? state.projects : [];
  const tasks = Array.isArray(state.tasks) ? state.tasks : [];
  const clientId = clients.some(item => item.id === input.clientId) ? input.clientId : '';
  const project = projects.find(item => item.id === input.projectId && (!clientId || item.clientId === clientId));
  const projectId = project?.id || '';
  const task = tasks.find(item => item.id === input.taskId && (!clientId || !item.clientId || item.clientId === clientId) && (!projectId || item.projectId === projectId));
  return { clientId, projectId, taskId: task?.id || '' };
}

function sanitizeDraft(input, state) {
  const candidate = input && typeof input === 'object' ? input : {};
  const kind = Object.hasOwn(KINDS, candidate.kind) ? candidate.kind : 'focus';
  const rawTarget = Number(candidate.targetMs);
  const targetMs = Number.isFinite(rawTarget)
    ? Math.max(MINUTE, Math.min(MAX_TARGET, Math.round(rawTarget / MINUTE) * MINUTE))
    : KINDS[kind].minutes * MINUTE;
  return {
    kind,
    goal: typeof candidate.goal === 'string' ? candidate.goal.replace(/[\u0000-\u001f\u007f]/g, '').slice(0, 500) : '',
    targetMs,
    ...linkedTags(candidate, state),
  };
}

export function getTimerDraft(state = {}) {
  let saved;
  try { saved = JSON.parse(globalThis.localStorage?.getItem(STORAGE_KEY) || 'null'); } catch { /* A damaged draft falls back to a new focus session. */ }
  return sanitizeDraft(saved, state);
}

export function timerElapsed(timer, now = Date.now()) {
  if (!timer) return 0;
  const accumulated = Math.max(0, Number(timer.elapsedMs) || 0);
  if (timer.status !== 'running') return accumulated;
  const resumed = Date.parse(timer.resumedAt || '');
  const timestamp = now instanceof Date ? now.getTime() : Number(now);
  return accumulated + (Number.isFinite(resumed) && Number.isFinite(timestamp) ? Math.max(0, timestamp - resumed) : 0);
}

function clock(milliseconds) {
  const seconds = Math.max(0, Math.floor((Number(milliseconds) || 0) / 1000));
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  return hours ? `${hours}:${String(minutes).padStart(2, '0')}:${String(seconds % 60).padStart(2, '0')}` : `${String(minutes).padStart(2, '0')}:${String(seconds % 60).padStart(2, '0')}`;
}

function durationLabel(milliseconds) {
  const minutes = Math.round(Math.max(0, Number(milliseconds) || 0) / MINUTE);
  return minutes >= 60 ? `${Math.floor(minutes / 60)}h ${minutes % 60}m` : `${minutes}m`;
}

function sessionDate(session) {
  const parsed = new Date(session.startedAt || session.start || session.endedAt || '');
  return Number.isFinite(parsed.getTime()) ? parsed : null;
}

export function sessionDayElapsed(session, timestamp) {
  const day = new Date(timestamp);
  const start = new Date(day.getFullYear(), day.getMonth(), day.getDate()).getTime();
  const end = new Date(day.getFullYear(), day.getMonth(), day.getDate() + 1).getTime();
  if (!Number.isFinite(start)) return 0;
  if (Array.isArray(session.segments)) return session.segments.reduce((sum, segment) => {
    const a = Date.parse(segment.start);
    const b = Date.parse(segment.end);
    return sum + (Number.isFinite(a) && Number.isFinite(b) ? Math.max(0, Math.min(end, b) - Math.max(start, a)) : 0);
  }, 0);
  // Legacy sessions have no intervals. Keep their recorded total on their start day.
  return sessionDate(session)?.toDateString() === day.toDateString() ? Math.max(0, Number(session.elapsedMs) || 0) : 0;
}

export default function TimerPage({ state = {}, mutate, now = Date.now() }) {
  const timer = state.timer || null;
  const [draft, setDraft] = useState(() => timer ? sanitizeDraft(timer, state) : getTimerDraft(state));
  const [duration, setDuration] = useState(() => String((timer?.targetMs || getTimerDraft(state).targetMs) / MINUTE));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const id = useId();
  const timerId = timer?.id || '';

  useEffect(() => {
    if (timer) {
      const next = sanitizeDraft(timer, state);
      setDraft(next);
      setDuration(String(next.targetMs / MINUTE));
    }
    setError('');
    setNotice('');
  }, [timerId]);

  useEffect(() => {
    if (!timer) {
      const saved = sanitizeDraft({ ...draft, targetMs: Number(duration) * MINUTE }, state);
      try { globalThis.localStorage?.setItem(STORAGE_KEY, JSON.stringify(saved)); } catch { /* Timer operation remains available when draft persistence is unavailable. */ }
    }
  }, [draft, duration, timerId, state.clients, state.projects, state.tasks]);

  const elapsed = timerElapsed(timer, now);
  const target = timer ? Math.max(MINUTE, Number(timer.targetMs) || MINUTE) : sanitizeDraft({ ...draft, targetMs: Number(duration) * MINUTE }, state).targetMs;
  const remaining = target - elapsed;
  const progress = Math.min(1, elapsed / target);
  const activeKind = Object.hasOwn(KINDS, timer?.kind) ? timer.kind : draft.kind;
  const currentKind = KINDS[activeKind];
  const clients = state.clients || [];
  const projects = (state.projects || []).filter(project => !draft.clientId || project.clientId === draft.clientId);
  const tasks = (state.tasks || []).filter(task => (!draft.clientId || !task.clientId || task.clientId === draft.clientId) && (!draft.projectId || task.projectId === draft.projectId));
  const sessions = useMemo(() => [...(state.sessions || [])].sort((a, b) => (sessionDate(b)?.getTime() || 0) - (sessionDate(a)?.getTime() || 0)), [state.sessions]);
  const currentDay = new Date(now).toDateString();
  const todaySessions = sessions.filter(session => sessionDayElapsed(session, now) > 0 || (session.elapsedMs === 0 && sessionDate(session)?.toDateString() === currentDay));
  const focusToday = todaySessions.filter(session => session.kind === 'focus').reduce((sum, session) => sum + sessionDayElapsed(session, now), 0);

  async function command(type, payload = {}, success = '') {
    if (busy) return;
    setBusy(true);
    setError('');
    setNotice('');
    try {
      if (typeof mutate !== 'function') throw new Error('The desktop workspace is unavailable. Reopen the app and try again.');
      await mutate({ type, payload });
      if (success) setNotice(success);
    } catch (failure) {
      setError(failure?.message || 'Your timer could not be updated. Please try again.');
    } finally { setBusy(false); }
  }

  function selectKind(kind) {
    setDraft(value => ({ ...value, kind, targetMs: KINDS[kind].minutes * MINUTE }));
    setDuration(String(KINDS[kind].minutes));
    setError('');
  }

  function changeClient(clientId) {
    setDraft(value => ({ ...value, clientId, projectId: '', taskId: '' }));
  }

  function changeProject(projectId) {
    const project = (state.projects || []).find(item => item.id === projectId);
    setDraft(value => ({ ...value, projectId, clientId: project?.clientId || value.clientId, taskId: '' }));
  }

  function changeTask(taskId) {
    const task = (state.tasks || []).find(item => item.id === taskId);
    const project = (state.projects || []).find(item => item.id === task?.projectId);
    setDraft(value => ({ ...value, taskId, projectId: task?.projectId || value.projectId, clientId: task?.clientId || project?.clientId || value.clientId }));
  }

  function submit(event) {
    event.preventDefault();
    if (timer) {
      const { goal, clientId, projectId, taskId } = sanitizeDraft(draft, state);
      void command('timer.update', { goal, clientId, projectId, taskId }, 'Session details saved.');
      return;
    }
    const minutes = Number(duration);
    if (!Number.isFinite(minutes) || minutes < 1 || minutes > 1440 || !Number.isInteger(minutes)) {
      setError('Enter a whole number of minutes between 1 and 1,440.');
      return;
    }
    const payload = sanitizeDraft({ ...draft, targetMs: minutes * MINUTE }, state);
    try { globalThis.localStorage?.setItem(STORAGE_KEY, JSON.stringify(payload)); } catch { /* Optional local draft persistence. */ }
    void command('timer.start', payload);
  }

  return (
    <div className="timer-page">
      <header className="timer-page-heading">
        <div><div className="timer-eyebrow">YOUR TIME, WITH INTENTION</div><h1>Timer</h1><p>A little structure for a more focused day.</p></div>
        <span className={`timer-state-pill ${timer?.status === 'running' ? 'is-running' : ''}`}><i />{timer ? timer.status === 'running' ? 'Session in progress' : 'Session paused' : 'Ready when you are'}</span>
      </header>

      <div className="timer-workspace">
        <section className={`timer-clock-card timer-kind-${activeKind}`} aria-label="Session timer">
          <div className="timer-kind-tabs" role="group" aria-label="Session type">
            {Object.entries(KINDS).map(([kind, value]) => <button key={kind} type="button" aria-pressed={activeKind === kind} disabled={Boolean(timer) || busy} onClick={() => selectKind(kind)}>{value.label}</button>)}
          </div>
          <div className="timer-ring">
            <svg viewBox="0 0 280 280" aria-hidden="true"><circle className="timer-ring-track" cx="140" cy="140" r="125" /><circle className="timer-ring-fill" cx="140" cy="140" r="125" strokeDasharray={2 * Math.PI * 125} strokeDashoffset={2 * Math.PI * 125 * (1 - progress)} /></svg>
            <div className="timer-ring-content"><span className="timer-clock-caption">{timer?.status === 'paused' ? 'PAUSED' : remaining < 0 ? 'EXTRA TIME' : `${currentKind.label.toUpperCase()} TIME`}</span><div className="timer-clock-value" role="timer" aria-label={`${remaining < 0 ? 'Overtime' : 'Time remaining'} ${clock(Math.abs(remaining))}`}>{remaining < 0 ? '+' : ''}{clock(Math.abs(remaining))}</div><span className="timer-target">{durationLabel(target)} session</span></div>
          </div>
          <p className="timer-kind-description">{timer?.goal || currentKind.description}</p>
          {timer ? <div className="timer-active-controls">
            <button type="button" className="timer-primary" disabled={busy} onClick={() => void command(timer.status === 'running' ? 'timer.pause' : 'timer.resume')}>{timer.status === 'running' ? 'Pause session' : 'Resume session'}</button>
            <div className="timer-secondary-controls"><button type="button" disabled={busy || target >= MAX_TARGET} onClick={() => void command('timer.extend', { minutes: Math.min(5, (MAX_TARGET - target) / MINUTE) })}>{target >= MAX_TARGET ? '24 hour limit' : `+ ${Math.min(5, (MAX_TARGET - target) / MINUTE)} minutes`}</button><button type="button" className="timer-stop" disabled={busy} onClick={() => void command('timer.stop', {}, 'Session saved to your history.')}>Finish session</button></div>
            <p className="timer-elapsed">{clock(elapsed)} elapsed{timer.status === 'paused' ? ' · paused time is excluded' : ''}</p>
          </div> : <div className="timer-ready-note"><span className="timer-ready-dot" />Choose your goal and start your session.</div>}
        </section>

        <section className="timer-details-card" aria-labelledby={`${id}-details`}>
          <div className="timer-card-title"><h2 id={`${id}-details`}>{timer ? 'Current session' : 'Set your intention'}</h2><span>{currentKind.label}</span></div>
          <form onSubmit={submit}>
            <label htmlFor={`${id}-goal`}>What are you working on?</label>
            <textarea id={`${id}-goal`} rows="3" maxLength="500" placeholder="Write down one thing you want to accomplish…" value={draft.goal} onChange={event => setDraft(value => ({ ...value, goal: event.target.value }))} disabled={busy} />
            {!timer && <><label htmlFor={`${id}-duration`}>Session length</label><div className="timer-duration-field"><input id={`${id}-duration`} type="number" min="1" max="1440" step="1" required value={duration} onChange={event => setDuration(event.target.value)} disabled={busy} /><span>minutes</span></div></>}
            <div className="timer-tags-heading">ORGANIZE YOUR TIME<span>Optional</span></div>
            <label htmlFor={`${id}-client`}>Client</label>
            <select id={`${id}-client`} value={draft.clientId} onChange={event => changeClient(event.target.value)} disabled={busy}><option value="">No client</option>{clients.map(client => <option key={client.id} value={client.id}>{client.name}</option>)}</select>
            <label htmlFor={`${id}-project`}>Project</label>
            <select id={`${id}-project`} value={draft.projectId} onChange={event => changeProject(event.target.value)} disabled={busy}><option value="">No project</option>{projects.map(project => <option key={project.id} value={project.id}>{project.name}</option>)}</select>
            <label htmlFor={`${id}-task`}>Task</label>
            <select id={`${id}-task`} value={draft.taskId} onChange={event => changeTask(event.target.value)} disabled={busy}><option value="">No task</option>{tasks.map(task => <option key={task.id} value={task.id}>{task.name}</option>)}</select>
            <button type="submit" className={timer ? 'timer-save-button' : 'timer-primary'} disabled={busy}>{busy ? 'Saving…' : timer ? 'Save session details' : `Start ${currentKind.label.toLowerCase()} session`}</button>
            <p className="timer-details-help">{timer ? 'Your session stays here when you switch views.' : 'Your timer keeps running when you switch views.'}</p>
          </form>
        </section>
      </div>

      <div className="timer-feedback" aria-live="polite">{error && <p role="alert" className="timer-error">{error}</p>}{notice && <p className="timer-notice">{notice}</p>}</div>

      <section className="timer-history" aria-labelledby={`${id}-history`}>
        <div className="timer-history-heading"><div><h2 id={`${id}-history`}>Session history</h2><p>Your completed sessions, with only active time counted.</p></div><div className="timer-day-summary"><strong>{durationLabel(focusToday)}</strong><span>focused today</span><i /><strong>{todaySessions.length}</strong><span>sessions</span></div></div>
        {sessions.length ? <div className="timer-history-list">{sessions.slice(0, 12).map((session, index) => {
          const date = sessionDate(session);
          const kind = KINDS[session.kind] || KINDS.focus;
          const project = (state.projects || []).find(item => item.id === session.projectId);
          return <article className="timer-history-row" key={session.id || `${session.startedAt}-${index}`}><span className={`timer-history-marker timer-marker-${session.kind}`} /><div className="timer-history-name"><strong>{session.goal || `${kind.label} session`}</strong><span>{kind.label}{project ? ` · ${project.name}` : ''}</span></div><time dateTime={date?.toISOString()}>{date ? date.toLocaleString(undefined, { month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit' }) : 'Date unavailable'}</time><strong className="timer-history-duration">{clock(session.elapsedMs)}</strong></article>;
        })}</div> : <div className="timer-history-empty"><span>◷</span><h3>Your first session starts here</h3><p>Finish a session and it will appear in your history.</p></div>}
      </section>
    </div>
  );
}
