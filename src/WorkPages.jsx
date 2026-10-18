import React, { useEffect, useMemo, useRef, useState } from 'react';
import { Search, Plus, Edit2, Trash2, X, Download, Upload, Users, Folder, CheckSquare, Sliders, CheckCircle, AlertCircle, Clock, Shield } from 'react-feather';
import './WorkPages.css';

const money = new Intl.NumberFormat('en-US', { style: 'currency', currency: 'USD', maximumFractionDigits: 2 });
const duration = value => { const minutes = Math.round(Math.max(0, Number(value) || 0) / 60000); return `${Math.floor(minutes / 60)}h ${String(minutes % 60).padStart(2, '0')}m`; };
const dayISO = value => { const date = new Date(value); return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, '0')}-${String(date.getDate()).padStart(2, '0')}`; };
const keywords = value => Array.isArray(value) ? value.join(', ') : String(value || '');
const splitKeywords = value => String(value || '').split(',').map(v => v.trim()).filter(Boolean);
const findName = (items, id, fallback = 'Unassigned') => items?.find(item => item.id === id)?.name || fallback;
const EMPTY_ARRAY = [];

function Notice({ error, success }) {
  if (!error && !success) return null;
  return <div className={`wp-notice ${error ? 'is-error' : 'is-success'}`} role={error ? 'alert' : 'status'}>{error ? <AlertCircle size={15} /> : <CheckCircle size={15} />}<span>{error || success}</span></div>;
}

function SelectField({ label, value, onChange, items = [], empty = 'None', disabled = false }) {
  return <label className="wp-field"><span>{label}</span><select value={value || ''} onChange={event => onChange(event.target.value)} disabled={disabled}><option value="">{empty}</option>{items.map(item => <option key={item.id} value={item.id}>{item.name}</option>)}</select></label>;
}

function TagsFields({ draft, setDraft, state, includeTask = false }) {
  const projects = state.projects || EMPTY_ARRAY;
  const availableProjects = projects.filter(project => !draft.clientId || project.clientId === draft.clientId);
  const availableTasks = (state.tasks || EMPTY_ARRAY).filter(task => (!draft.clientId || task.clientId === draft.clientId) && (!draft.projectId || task.projectId === draft.projectId));
  return <div className="wp-form-grid"><SelectField label="Client" value={draft.clientId} items={state.clients} onChange={clientId => setDraft({ ...draft, clientId, projectId: '', taskId: '' })} /><SelectField label="Project" value={draft.projectId} items={availableProjects} onChange={projectId => { const project = projects.find(item => item.id === projectId); setDraft({ ...draft, projectId, clientId: project?.clientId || draft.clientId || '', taskId: '' }); }} />{includeTask && <SelectField label="Task" value={draft.taskId} items={availableTasks} onChange={taskId => { const task = state.tasks?.find(item => item.id === taskId); setDraft({ ...draft, taskId, projectId: task?.projectId || draft.projectId || '', clientId: task?.clientId || draft.clientId || '' }); }} />}</div>;
}

export function WorkPage({ kind, state, mutate }) {
  const safeKind = ['clients', 'projects', 'tasks'].includes(kind) ? kind : 'projects';
  const singular = safeKind.slice(0, -1);
  const title = safeKind[0].toUpperCase() + safeKind.slice(1);
  const Icon = safeKind === 'clients' ? Users : safeKind === 'projects' ? Folder : CheckSquare;
  const [search, setSearch] = useState('');
  const [draft, setDraft] = useState(null);
  const [error, setError] = useState('');
  const [success, setSuccess] = useState('');
  const [busy, setBusy] = useState(false);
  const editor = useRef(null), opener = useRef(null);
  const editorOpen = !!draft;
  useEffect(() => {
    if (!editorOpen) return;
    const dialog = editor.current, origin = opener.current;
    const focusable = () => [...dialog.querySelectorAll('button:not([disabled]),input:not([disabled]),select:not([disabled]),textarea:not([disabled]),[tabindex="0"]')];
    dialog.querySelector('input')?.focus();
    const keyboard = event => {
      if (event.key === 'Escape') { event.preventDefault(); setDraft(null); setError(''); }
      if (event.key !== 'Tab') return;
      const elements = focusable(), first = elements[0], last = elements.at(-1);
      if (!first) return;
      if (event.shiftKey && (document.activeElement === first || !dialog.contains(document.activeElement))) { event.preventDefault(); last.focus(); }
      else if (!event.shiftKey && (document.activeElement === last || !dialog.contains(document.activeElement))) { event.preventDefault(); first.focus(); }
    };
    document.addEventListener('keydown', keyboard);
    return () => { document.removeEventListener('keydown', keyboard); if (origin?.isConnected) origin.focus(); };
  }, [editorOpen]);
  useEffect(() => { setSearch(''); setDraft(null); setError(''); setSuccess(''); }, [safeKind]);
  const items = state[safeKind] || EMPTY_ARRAY;
  const filtered = items.filter(item => `${item.name} ${keywords(item.keywords)} ${findName(state.clients, item.clientId, '')} ${findName(state.projects, item.projectId, '')}`.toLowerCase().includes(search.toLowerCase()));
  const open = (item, trigger) => { opener.current = trigger || document.activeElement; setError(''); setSuccess(''); setDraft({ id: item?.id || '', name: item?.name || '', hourlyRate: item?.hourlyRate ?? 0, clientId: item?.clientId || '', projectId: item?.projectId || '', keywords: keywords(item?.keywords) }); };
  const save = async event => {
    event.preventDefault(); setError(''); setBusy(true);
    try {
      const payload = { name: draft.name.trim(), ...(draft.id ? { id: draft.id } : {}) };
      if (!payload.name) throw new Error('Enter a name.');
      if (safeKind === 'clients') payload.hourlyRate = Number(draft.hourlyRate || 0);
      else { payload.clientId = draft.clientId || ''; payload.keywords = splitKeywords(draft.keywords); if (safeKind === 'tasks') payload.projectId = draft.projectId || ''; }
      await mutate({ type: `${singular}.upsert`, payload });
      setSuccess(`${singular[0].toUpperCase() + singular.slice(1)} saved.`); setDraft(null);
    } catch (err) { setError(err.message || 'Could not save this item.'); } finally { setBusy(false); }
  };
  const remove = async item => {
    if (!window.confirm(`Delete “${item.name}”? Existing time entries will keep their recorded time.`)) return;
    setError(''); setSuccess('');
    try { await mutate({ type: `${singular}.delete`, payload: { id: item.id } }); if (draft?.id === item.id) setDraft(null); setSuccess(`${singular[0].toUpperCase() + singular.slice(1)} deleted.`); } catch (err) { setError(err.message || 'Could not delete this item.'); }
  };
  const entityTimes = useMemo(() => { const totals = new Map(); for (const entry of state.entries || EMPTY_ARRAY) { if (entry.status !== 'approved') continue; const id = entry[`${singular}Id`]; if (!id) continue; totals.set(id, (totals.get(id) || 0) + Math.max(0, Date.parse(entry.end) - Date.parse(entry.start) || 0)); } return totals; }, [state.entries, singular]);
  return <section className="wp-page">
    <header className="wp-header"><div><div className="wp-eyebrow">WORKSPACE</div><h1>{title}</h1><p>Organize your time around the work that matters.</p></div><button className="wp-button is-primary" onClick={event => open(null, event.currentTarget)}><Plus size={16} />New {singular}</button></header>
    {!draft && <Notice error={error} success={success} />}
    <div className="wp-toolbar"><label className="wp-search"><Search size={16} /><input aria-label={`Search ${safeKind}`} placeholder={`Search ${safeKind}…`} value={search} onChange={event => setSearch(event.target.value)} /></label><span className="wp-muted">{filtered.length} {filtered.length === 1 ? singular : safeKind}</span></div>
    {draft && <div className="wp-editor-backdrop" onMouseDown={event => { if (event.target === event.currentTarget) { setDraft(null); setError(''); } }}><form className="wp-editor" ref={editor} role="dialog" aria-modal="true" aria-labelledby="wp-editor-title" onSubmit={save}><div className="wp-editor-heading"><h2 id="wp-editor-title">{draft.id ? 'Edit' : 'New'} {singular}</h2><button className="wp-icon-button" type="button" aria-label="Close editor" onClick={() => { setDraft(null); setError(''); }}><X size={17} /></button></div><Notice error={error} success={success} /><label className="wp-field"><span>Name</span><input autoFocus required maxLength={160} value={draft.name} onChange={event => setDraft({ ...draft, name: event.target.value })} placeholder={`${singular[0].toUpperCase() + singular.slice(1)} name`} /></label>
      {safeKind === 'clients' ? <label className="wp-field"><span>Hourly rate · USD</span><input type="number" min="0" max="100000" step="0.01" value={draft.hourlyRate} onChange={event => setDraft({ ...draft, hourlyRate: event.target.value })} /><small>Used for billable time in reports.</small></label> : <>{safeKind === 'projects' ? <SelectField label="Client" value={draft.clientId} items={state.clients} onChange={clientId => setDraft({ ...draft, clientId })} /> : <TagsFields draft={draft} setDraft={setDraft} state={state} />}<label className="wp-field"><span>Keywords</span><input value={draft.keywords} maxLength={2000} onChange={event => setDraft({ ...draft, keywords: event.target.value })} placeholder="design, website, launch" /><small>Separate keywords with commas. Keywords help match recorded window titles.</small></label></>}
      <div className="wp-form-actions"><button type="button" className="wp-button" onClick={() => setDraft(null)}>Cancel</button><button className="wp-button is-primary" disabled={busy}>{busy ? 'Saving…' : `Save ${singular}`}</button></div></form></div>}
    <div className={`wp-list wp-entity-table wp-entity-table--${safeKind}`} role="list">
      <div className="wp-list-heading" aria-hidden="true"><span>{singular[0].toUpperCase()+singular.slice(1)}</span><span>{safeKind === 'clients' ? 'Projects' : 'Client'}</span><span>{safeKind === 'clients' ? 'Hourly rate' : safeKind === 'tasks' ? 'Project' : 'Keywords'}</span><span>Time spent</span><span /></div>
      {filtered.map(item => <article key={item.id} className="wp-entity" role="listitem"><div className="wp-entity-name"><span className="wp-entity-icon"><Icon size={15} /></span><div className="wp-entity-info"><h2>{item.name}</h2></div></div><div className="wp-entity-meta">{safeKind === 'clients' ? `${(state.projects || EMPTY_ARRAY).filter(project => project.clientId === item.id).length} projects` : findName(state.clients, item.clientId, 'No client')}</div><div className="wp-entity-meta">{safeKind === 'clients' ? `${money.format(Number(item.hourlyRate) || 0)} / hour` : safeKind === 'tasks' ? <>{findName(state.projects, item.projectId, 'No project')}{keywords(item.keywords) && <span className="wp-keywords">{keywords(item.keywords)}</span>}</> : keywords(item.keywords) || '—'}</div><div className="wp-entity-time"><strong>{duration(entityTimes.get(item.id))}</strong><span>approved time</span></div><div className="wp-row-actions"><button className="wp-icon-button" aria-label={`Edit ${item.name}`} onClick={event => open(item, event.currentTarget)}><Edit2 size={16} /></button><button className="wp-icon-button is-danger" aria-label={`Delete ${item.name}`} onClick={() => remove(item)}><Trash2 size={16} /></button></div></article>)}
    </div>
    {!filtered.length && <div className="wp-empty"><Icon size={27} /><h2>{search ? 'No matches' : `Your ${safeKind} start here`}</h2><p>{search ? 'Try another name or keyword.' : `Add a ${singular} to tag time entries and organize your reports.`}</p>{!search && <button className="wp-button" onClick={event => open(null, event.currentTarget)}><Plus size={15} />Add {singular}</button>}</div>}
  </section>;
}

export function ReportsPage({ state }) {
  const [start, setStart] = useState(() => { const date = new Date(); date.setDate(date.getDate() - 6); return dayISO(date); });
  const [end, setEnd] = useState(() => dayISO(new Date()));
  const [status, setStatus] = useState('approved');
  const [group, setGroup] = useState('project');
  const [error, setError] = useState('');
  const [success, setSuccess] = useState('');
  const rangeError = start && end && start > end ? 'The end date must be on or after the start date.' : '';
  const rows = useMemo(() => {
    if (rangeError || !start || !end) return [];
    const from = new Date(`${start}T00:00:00`).getTime(); const untilDate = new Date(`${end}T00:00:00`); untilDate.setDate(untilDate.getDate() + 1); const until = untilDate.getTime();
    return (state.entries || EMPTY_ARRAY).filter(entry => status === 'all' || entry.status === status).flatMap(entry => { const ms = Math.max(0, Math.min(Date.parse(entry.end), until) - Math.max(Date.parse(entry.start), from)); return ms > 0 ? [{ ...entry, reportMs: ms }] : []; });
  }, [state.entries, start, end, status, rangeError]);
  const total = rows.reduce((sum, entry) => sum + entry.reportMs, 0);
  const billable = rows.filter(entry => entry.billable).reduce((sum, entry) => sum + entry.reportMs, 0);
  const amount = entry => entry.billable ? entry.reportMs / 3600000 * (Number(state.clients?.find(client => client.id === entry.clientId)?.hourlyRate) || 0) : 0;
  const totalValue = rows.reduce((sum, entry) => sum + amount(entry), 0);
  const groups = new Map();
  rows.forEach(entry => { const key = group === 'app' ? (entry.app || 'Unknown app') : group === 'client' ? (entry.clientId || '') : (entry.projectId || ''); const name = group === 'app' ? key : group === 'client' ? findName(state.clients, key, 'No client') : findName(state.projects, key, 'No project'); const existing = groups.get(key) || { key, name, ms: 0, count: 0, value: 0 }; existing.ms += entry.reportMs; existing.count++; existing.value += amount(entry); groups.set(key, existing); });
  const grouped = [...groups.values()].sort((a, b) => b.ms - a.ms);
  const preset = days => { const last = new Date(); const first = new Date(); first.setDate(first.getDate() - days + 1); setStart(dayISO(first)); setEnd(dayISO(last)); };
  const exportCSV = async () => { setError(''); setSuccess(''); try { if (!window.rize?.exportData) throw new Error('Open the Windows desktop app to save a CSV file.'); const result = await window.rize.exportData('csv'); if (!result?.canceled) setSuccess('Workspace CSV export complete.'); } catch (err) { setError(err.message || 'Export failed.'); } };
  return <section className="wp-page"><header className="wp-header"><div><div className="wp-eyebrow">UNDERSTAND YOUR TIME</div><h1>Reports</h1><p>A clear view of where your workday goes.</p></div><button className="wp-button" onClick={exportCSV}><Download size={16} />Export workspace CSV</button></header><Notice error={error || rangeError} success={success} />
    <div className="wp-report-filters"><label className="wp-field"><span>From</span><input type="date" value={start} onChange={event => setStart(event.target.value)} /></label><label className="wp-field"><span>To</span><input type="date" value={end} onChange={event => setEnd(event.target.value)} /></label><label className="wp-field"><span>Entry status</span><select value={status} onChange={event => setStatus(event.target.value)}><option value="approved">Approved</option><option value="pending">Pending review</option><option value="all">All entries</option></select></label><div className="wp-preset-buttons"><button className="wp-button" onClick={() => preset(1)}>Today</button><button className="wp-button" onClick={() => preset(7)}>7 days</button><button className="wp-button" onClick={() => preset(30)}>30 days</button></div></div>
    <div className="wp-metrics"><div><span>Tracked time</span><strong>{duration(total)}</strong><small>{rows.length} entries in this range</small></div><div><span>Billable time</span><strong>{duration(billable)}</strong><small>{total ? Math.round(billable / total * 100) : 0}% of tracked time</small></div><div><span>Billable value</span><strong>{money.format(totalValue)}</strong><small>Current client hourly rates · USD</small></div></div>
    <div className="wp-report-panel"><div className="wp-section-heading"><h2>Time breakdown</h2><label className="wp-inline-select"><span>Group by</span><select value={group} onChange={event => setGroup(event.target.value)}><option value="project">Project</option><option value="client">Client</option><option value="app">Application</option></select></label></div>
    {grouped.length ? <div className="wp-breakdown">{grouped.map(row => <div key={row.key} className="wp-breakdown-row"><div><strong>{row.name}</strong><span>{row.count} {row.count === 1 ? 'entry' : 'entries'}</span></div><div className="wp-bar"><span style={{ width: `${total ? row.ms / total * 100 : 0}%` }} /></div><strong className="wp-breakdown-duration">{duration(row.ms)}</strong><span>{money.format(row.value)}</span></div>)}</div> : <div className="wp-empty"><Clock size={26} /><h2>No time in this range</h2><p>{status === 'approved' ? 'Approve recorded entries in Calendar or choose another status.' : 'Try another date range or begin tracking your work.'}</p></div>}</div><p className="wp-footnote">Report time is clipped to the selected local dates. Billable value uses current client rates. CSV export includes the entire workspace.</p></section>;
}

export function SettingsPage({ state, mutate, status }) {
  const settings = state.settings || {};
  const settingsKey = JSON.stringify(settings);
  const [draft, setDraft] = useState({ ...settings });
  const [ruleDraft, setRuleDraft] = useState(null);
  const [error, setError] = useState('');
  const [success, setSuccess] = useState('');
  const [busy, setBusy] = useState(false);
  useEffect(() => { setDraft({ ...settings }); }, [settingsKey]);
  const save = async event => { event.preventDefault(); setError(''); setSuccess(''); setBusy(true); try { await mutate({ type: 'settings.update', payload: { displayName: String(draft.displayName || '').trim(), trackingEnabled: !!draft.trackingEnabled, idleSeconds: Number(draft.idleSeconds) || 300, scheduleEnabled: !!draft.scheduleEnabled, scheduleStart: draft.scheduleStart || '09:00', scheduleEnd: draft.scheduleEnd || '18:00', workDays: draft.workDays || [1, 2, 3, 4, 5] } }); setSuccess('Settings saved.'); } catch (err) { setError(err.message || 'Could not save settings.'); } finally { setBusy(false); } };
  const saveRule = async event => { event.preventDefault(); setError(''); setSuccess(''); try { await mutate({ type: 'rule.upsert', payload: { ...(ruleDraft.id ? { id: ruleDraft.id } : {}), name: ruleDraft.name.trim(), keyword: ruleDraft.keyword.trim(), action: ruleDraft.action, clientId: ruleDraft.clientId || '', projectId: ruleDraft.projectId || '', taskId: ruleDraft.taskId || '' } }); setRuleDraft(null); setSuccess('Rule saved.'); } catch (err) { setError(err.message || 'Could not save rule.'); } };
  const deleteRule = async rule => { if (!window.confirm(`Delete the rule “${rule.name}”?`)) return; try { await mutate({ type: 'rule.delete', payload: { id: rule.id } }); setSuccess('Rule deleted.'); setError(''); } catch (err) { setError(err.message || 'Could not delete rule.'); } };
  const bridgeAction = async action => { setError(''); setSuccess(''); try { if (!window.rize) throw new Error('Open the Windows desktop app to manage backup files.'); const result = action === 'import' ? await window.rize.importData() : await window.rize.exportData(action); if (!result?.canceled) setSuccess(action === 'import' ? 'Workspace import complete.' : 'Export complete.'); } catch (err) { setError(err.message || 'File operation failed.'); } };
  const toggleDay = day => { const days = draft.workDays || [1, 2, 3, 4, 5]; setDraft({ ...draft, workDays: days.includes(day) ? days.filter(value => value !== day) : [...days, day].sort() }); };
  const tracking = status?.tracking || {};
  return <section className="wp-page wp-settings"><header className="wp-header"><div><div className="wp-eyebrow">MAKE IT YOURS</div><h1>Settings</h1><p>Your workspace, tracking preferences, and data.</p></div><span className="wp-local-badge"><Shield size={13} />Device settings</span></header><Notice error={error} success={success} />
    <form onSubmit={save}><section className="wp-settings-panel"><div className="wp-section-heading"><h2>Profile</h2><Users size={17} /></div><label className="wp-field"><span>Display name</span><input required maxLength={120} value={draft.displayName || ''} onChange={event => setDraft({ ...draft, displayName: event.target.value })} /></label></section>
      <section className="wp-settings-panel"><div className="wp-section-heading"><h2>Automatic tracking</h2><Sliders size={17} /></div><label className="wp-toggle"><div><strong>Record active applications</strong><span>Collect the active app and window title while you work.</span></div><input type="checkbox" checked={!!draft.trackingEnabled} disabled={!!state.demo} onChange={event => setDraft({ ...draft, trackingEnabled: event.target.checked })} /></label><p className="wp-tracking-status"><span className={`wp-status-dot ${tracking.enabled && tracking.available ? 'is-active' : ''}`} />{state.demo ? 'Tracking is disabled in the sample workspace.' : tracking.message || (tracking.available ? 'Windows activity capture is available.' : 'Activity capture is available in the Windows desktop app.')}</p><label className="wp-field wp-narrow"><span>Mark as idle after</span><div className="wp-input-suffix"><input type="number" min="30" max="3600" step="30" value={draft.idleSeconds ?? 300} onChange={event => setDraft({ ...draft, idleSeconds: event.target.value })} /><span>seconds</span></div><small>Idle time is excluded from automatic entries.</small></label><p className="wp-footnote">Raw application names and window titles stay on this computer. Recording excludes screenshots and keystrokes. Approved descriptions and tags are shared only when you enable team sync. Exclude sensitive apps with rules below.</p></section>
      <section className="wp-settings-panel"><div className="wp-section-heading"><h2>Work schedule</h2><Clock size={17} /></div><label className="wp-toggle"><div><strong>Track during scheduled hours only</strong><span>Applies to automatic tracking in your computer’s local time.</span></div><input type="checkbox" checked={!!draft.scheduleEnabled} onChange={event => setDraft({ ...draft, scheduleEnabled: event.target.checked })} /></label><div className="wp-week-days" aria-label="Work days">{['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'].map((day, index) => <button className={(draft.workDays || [1, 2, 3, 4, 5]).includes(index) ? 'is-selected' : ''} key={day} type="button" aria-pressed={(draft.workDays || [1, 2, 3, 4, 5]).includes(index)} onClick={() => toggleDay(index)}>{day}</button>)}</div><div className="wp-form-grid"><label className="wp-field"><span>Start</span><input type="time" value={draft.scheduleStart || '09:00'} onChange={event => setDraft({ ...draft, scheduleStart: event.target.value })} /></label><label className="wp-field"><span>End</span><input type="time" value={draft.scheduleEnd || '18:00'} onChange={event => setDraft({ ...draft, scheduleEnd: event.target.value })} /></label></div></section><div className="wp-settings-save"><button className="wp-button is-primary" disabled={busy}>{busy ? 'Saving…' : 'Save preferences'}</button></div>
    </form>
    <section className="wp-settings-panel"><div className="wp-section-heading"><div><h2>Activity rules</h2><p>Match app names or window titles to categorize and tag new activity.</p></div><button className="wp-button" onClick={() => { setRuleDraft({ name: '', keyword: '', action: 'productive', clientId: '', projectId: '', taskId: '' }); setError(''); }}><Plus size={15} />Add rule</button></div>
      {ruleDraft && <form className="wp-rule-editor" onSubmit={saveRule}><div className="wp-form-grid"><label className="wp-field"><span>Rule name</span><input required maxLength={160} value={ruleDraft.name} onChange={event => setRuleDraft({ ...ruleDraft, name: event.target.value })} /></label><label className="wp-field"><span>Match keyword</span><input required maxLength={300} value={ruleDraft.keyword} placeholder="e.g. Figma" onChange={event => setRuleDraft({ ...ruleDraft, keyword: event.target.value })} /></label></div><label className="wp-field"><span>Action</span><select value={ruleDraft.action} onChange={event => setRuleDraft({ ...ruleDraft, action: event.target.value })}><option value="productive">Mark productive</option><option value="neutral">Mark neutral</option><option value="distracting">Mark distracting</option><option value="exclude">Exclude from tracking</option></select></label>{ruleDraft.action !== 'exclude' && <TagsFields draft={ruleDraft} setDraft={setRuleDraft} state={state} includeTask />}<div className="wp-form-actions"><button className="wp-button" type="button" onClick={() => setRuleDraft(null)}>Cancel</button><button className="wp-button is-primary">Save rule</button></div></form>}
      <div className="wp-rules">{(state.rules || EMPTY_ARRAY).map(rule => <div className="wp-rule" key={rule.id}><div><strong>{rule.name}</strong><span>Contains “{rule.keyword}” · {rule.action === 'exclude' ? 'Excluded' : rule.action}{rule.projectId && ` · ${findName(state.projects, rule.projectId)}`}</span></div><div className="wp-row-actions"><button className="wp-icon-button" aria-label={`Edit rule ${rule.name}`} onClick={() => setRuleDraft({ ...rule, clientId: rule.clientId || '', projectId: rule.projectId || '', taskId: rule.taskId || '' })}><Edit2 size={15} /></button><button className="wp-icon-button is-danger" aria-label={`Delete rule ${rule.name}`} onClick={() => deleteRule(rule)}><Trash2 size={15} /></button></div></div>)}{!state.rules?.length && <p className="wp-muted wp-rules-empty">No rules yet. Add one to organize or exclude new activity.</p>}</div><p className="wp-footnote">Rules affect future activity. Review existing entries in Calendar to change their tags.</p></section>
    <section className="wp-settings-panel"><div className="wp-section-heading"><h2>Data & backups</h2><Shield size={17} /></div><p className="wp-muted">Save a JSON backup to preserve your workspace, or export all time entries to a spreadsheet. Import replaces the current workspace after confirmation.</p><div className="wp-backup-actions"><button className="wp-button" onClick={() => bridgeAction('json')}><Download size={15} />Export JSON backup</button><button className="wp-button" onClick={() => bridgeAction('csv')}><Download size={15} />Export workspace CSV</button><button className="wp-button" onClick={() => bridgeAction('import')}><Upload size={15} />Import backup</button></div><p className="wp-footnote">Open Team to connect an account and your own sync server. AI requests and integrations require provider setup; approve sharing before sending team data to an AI provider.</p></section>
  </section>;
}
