const test = require('node:test');
const assert = require('node:assert/strict');
const path = require('node:path');
const { parseHTML } = require('linkedom');
const { window } = parseHTML('<!doctype html><html><body></body></html>');
window.document.oninput = null;
for (const name of ['window','document','HTMLElement','HTMLInputElement','HTMLTextAreaElement','HTMLSelectElement','Event','MouseEvent','Node']) if (window[name]) Object.defineProperty(globalThis,name,{value:window[name],configurable:true,writable:true});
Object.defineProperty(globalThis,'navigator',{value:{userAgent:'Linkedom business integration test'},configurable:true});
globalThis.IS_REACT_ACT_ENVIRONMENT = true;
const React = require('react');
const { act } = React;
const { createRoot } = require('react-dom/client');
const { renderToStaticMarkup } = require('react-dom/server');
const compiled = require('esbuild').buildSync({entryPoints:[path.join(__dirname,'../src/BusinessPages.jsx')],bundle:true,write:false,format:'cjs',platform:'node',external:['react','react-dom','react-feather'],loader:{'.css':'empty'}});
const businessModule = { exports: {} };
new Function('require','module','exports',compiled.outputFiles[0].text)(require,businessModule,businessModule.exports);
const { InvoicesPage, ProfitabilityPage, ResourcingPage, TimesheetsPage, localTimesheet, parseAllocationCSV, moneyToMinor, mondayDate, localDate } = businessModule.exports;
const state = () => ({settings:{displayName:'Test User'},clients:[{id:'c1',name:'Acme',hourlyRate:125}],projects:[{id:'p1',clientId:'c1',name:'Launch'}],tasks:[],entries:[],business:{profile:{name:'Test Studio'},clientTerms:[{id:'terms',clientId:'c1',currency:'JPY',paymentDays:14}],contracts:[],invoices:[],allocations:[]}});
function button(container,text) { const node=[...container.querySelectorAll('button')].find(el=>el.textContent.trim()===text); assert.ok(node,`Expected button: ${text}`); return node; }
async function click(element) { await act(async()=>element.dispatchEvent(new window.Event('click',{bubbles:true}))); }
async function submit(form) { await act(async()=>form.dispatchEvent(new window.Event('submit',{bubbles:true,cancelable:true}))); }
async function input(element,value) { const d=Object.getOwnPropertyDescriptor(Object.getPrototypeOf(element),'value'); assert.equal(typeof d?.set,'function'); d.set.call(element,value); await act(async()=>element.dispatchEvent(new window.Event('input',{bubbles:true}))); }
async function mount(Page,props) { const container=document.createElement('div'); document.body.append(container); const root=createRoot(container); await act(async()=>root.render(React.createElement(Page,props))); return {container,cleanup:async()=>{await act(async()=>root.unmount());container.remove();}}; }

test('allocation CSV handles quoted commas and rejects unknown IDs, invalid dates and duplicates before saving',()=>{
  const options={projects:[{id:'p1'}],members:[{id:'local'}]};
  const header='memberId,memberName,projectId,week,hours,capacityHours\r\n';
  assert.deepEqual(parseAllocationCSV(header+'local,"Doe, Jane",p1,2026-10-12,12.5,40\r\n',options),[{memberId:'local',memberName:'Doe, Jane',projectId:'p1',week:'2026-10-12',hours:12.5,capacityHours:40}]);
  assert.throws(()=>parseAllocationCSV(header+'missing,Jane,p1,2026-10-12,8,40',options),/current team member/);
  assert.throws(()=>parseAllocationCSV(header+'local,Jane,missing,2026-10-12,8,40',options),/projectId/);
  assert.throws(()=>parseAllocationCSV(header+'local,Jane,p1,2026-10-13,8,40',options),/Monday/);
  assert.throws(()=>parseAllocationCSV(header+'local,Jane,p1,2026-02-30,8,40',options),/Monday/);
  assert.throws(()=>parseAllocationCSV(header+'local,Jane,p1,2026-10-12,169,40',options),/between 0 and 168/);
  assert.throws(()=>parseAllocationCSV(header+'local,Jane,p1,2026-10-12,8,40\nlocal,Jane,p1,2026-10-12,4,40',options),/duplicate/);
  assert.throws(()=>parseAllocationCSV(header+'local,"Jane,p1,2026-10-12,8,40',options),/unclosed/);
});

test('currency input converts zero, two and three digit currencies to integer minor units',()=>{
  assert.equal(moneyToMinor('10.25','USD'),1025);assert.equal(moneyToMinor('10','JPY'),10);assert.equal(moneyToMinor('10.125','KWD'),10125);assert.throws(()=>moneyToMinor('-1','USD'),/non-negative/);assert.throws(()=>moneyToMinor('NaN','USD'),/non-negative/);
});

test('personal timesheets split actual intervals at midnight and keep pending separate',()=>{
  const s=state(),start=new Date(2026,9,12,23,30),end=new Date(2026,9,13,1,15);
  s.entries=[{projectId:'p1',clientId:'c1',status:'approved',billable:true,start:start.toISOString(),end:end.toISOString()},{projectId:'p1',clientId:'c1',status:'pending',start:new Date(2026,9,13,2).toISOString(),end:new Date(2026,9,13,3).toISOString()},{status:'rejected',start:start.toISOString(),end:end.toISOString()}];
  const rows=localTimesheet(s,'2026-10-12','2026-10-18'),approved=rows.find(r=>r.status==='approved'),pending=rows.find(r=>r.status==='pending');
  assert.equal(approved.days['2026-10-12'],0.5);assert.equal(approved.days['2026-10-13'],1.25);assert.equal(approved.billableHours,1.75);assert.equal(pending.totalHours,1);assert.equal(rows.length,2);assert.equal(localTimesheet(s,'2026-10-18','2026-10-12').length,0);
});

test('business empty views show no fabricated financial totals or team members',()=>{
  const s=state();
  const finance=renderToStaticMarkup(React.createElement(ProfitabilityPage,{state:s,mutate(){}}));
  assert.match(finance,/Ready to calculate/);assert.match(finance,/No estimated money is shown/);assert.doesNotMatch(finance,/\$[1-9]/);
  const resource=renderToStaticMarkup(React.createElement(ResourcingPage,{state:s,mutate(){},cloud:{authenticated:false}}));
  assert.match(resource,/No sample people or capacity/);assert.doesNotMatch(resource,/Load team members/);
  const sheet=renderToStaticMarkup(React.createElement(TimesheetsPage,{state:s,cloud:{authenticated:false}}));assert.match(sheet,/No entries this week/);
});

test('failed invoice draft save retains user values and uses client currency terms on retry',async()=>{
  let fail=true;const commands=[];const app=await mount(InvoicesPage,{state:state(),mutate:async command=>{commands.push(command);if(fail)throw new Error('Disk write unavailable');return state();}});
  try {
    await click(button(app.container,'New invoice'));
    const notes=app.container.querySelector('textarea');await input(notes,'Keep this draft for review');
    await submit(app.container.querySelector('form'));
    assert.match(app.container.querySelector('[role="alert"]').textContent,/Disk write unavailable/);assert.equal(app.container.querySelector('textarea').value,'Keep this draft for review');
    assert.equal(commands[0].type,'invoice.create');assert.equal(commands[0].payload.currency,'JPY');assert.equal(commands[0].payload.paymentDays,14);assert.equal(commands[0].payload.discountMinor,0);assert.ok(commands[0].payload.issueDate);
    fail=false;await submit(app.container.querySelector('form'));assert.equal(app.container.querySelector('textarea'),null);assert.match(app.container.querySelector('[role="status"]').textContent,/Invoice draft saved/);
  } finally {await app.cleanup();}
});

test('invoice PDF cancellation has no success claim and sent status explicitly does not email',async()=>{
  const s=state(),commands=[],calls=[];s.business.invoices=[{id:'i1',number:'INV-001',status:'draft',clientId:'c1',clientName:'Acme',issueDate:'2026-10-17',dueDate:'2026-10-31',currency:'USD',currencyDigits:2,issuer:{name:'Test Studio'},billTo:{},lines:[{id:'l1',description:'Approved design work',quantityHours:2,rate:125,amountMinor:25000}],subtotalMinor:25000,totalMinor:25000,outstandingMinor:25000}];
  window.rize={invoicePdf:async payload=>{calls.push(payload);return{canceled:true};}};
  const app=await mount(InvoicesPage,{state:s,mutate:async command=>{commands.push(command);return s;}});
  try {await click(button(app.container,'INV-001'));await click(button(app.container,'Save PDF'));assert.deepEqual(calls,[{id:'i1'}]);assert.equal(app.container.querySelector('[role="status"]'),null);await click(button(app.container,'Mark as sent'));assert.equal(commands[0].type,'invoice.markSent');assert.match(app.container.textContent,/This app has not emailed the invoice/);assert.match(app.container.textContent,/Approved design work/);} finally {delete window.rize;await app.cleanup();}
});

test('team timesheets request the selected week and render only the server aggregates',async()=>{
  const calls=[],week=mondayDate(new Date());
  const app=await mount(TimesheetsPage,{state:state(),cloud:{authenticated:true,teamId:'team1'},cloudRequest:async(action,payload)=>{calls.push({action,payload});return{startDate:week,endDate:week,timezone:'Asia/Kolkata',totals:{durationMs:7200000,billableMs:3600000},utilization:[{userId:'u1',name:'Actual User',durationMs:7200000,billableMs:3600000,capacityMs:144000000,utilization:0.05}],timesheet:[{userId:'u1',date:week,durationMs:7200000,billableMs:3600000,entryCount:1}],capacityBasis:'Weekly configured capacity.'};}});
  try {await click([...app.container.querySelectorAll('[role="tab"]')].find(el=>el.textContent==='Team timesheets'));assert.doesNotMatch(app.container.textContent,/Actual User/);await click(button(app.container,'Load team timesheets'));assert.equal(calls[0].action,'analytics');assert.equal(calls[0].payload.startDate,week);assert.ok(calls[0].payload.timezone);assert.match(app.container.textContent,/Actual User/);assert.match(app.container.textContent,/5.0%/);assert.match(app.container.textContent,/Asia\/Kolkata/);} finally {await app.cleanup();}
});

test('recording a partial payment sends an integer minor-unit amount and retains errors',async()=>{
  const s=state(),commands=[];s.business.invoices=[{id:'i2',number:'INV-002',status:'sent',clientName:'Acme',issueDate:'2026-10-17',dueDate:'2026-10-31',currency:'KWD',currencyDigits:3,issuer:{},billTo:{},lines:[],totalMinor:20000,paidMinor:0,outstandingMinor:20000}];
  const app=await mount(InvoicesPage,{state:s,mutate:async command=>{commands.push(command);throw new Error('Payment persistence failed');}});
  try {await click(button(app.container,'INV-002'));const form=app.container.querySelector('form');await input(form.querySelector('input[type="number"]'),'10.125');await submit(form);assert.equal(commands[0].type,'invoice.payment');assert.equal(commands[0].payload.amountMinor,10125);assert.equal(commands[0].payload.id,'i2');assert.match(app.container.querySelector('[role="alert"]').textContent,/Payment persistence failed/);assert.equal(form.querySelector('input[type="number"]').value,'10.125');} finally {await app.cleanup();}
});

test('financial report renders the native currency totals and contract budget without inferred amounts',async()=>{
  window.rize={businessReport:async()=>({currencies:[{currency:'USD',currencyDigits:2,totalHours:8,billableHours:6,revenueMinor:75000,costMinor:32000,profitMinor:43000}],projects:[{projectId:'p1',projectName:'Launch',clientName:'Acme',currency:'USD',currencyDigits:2,totalHours:8,revenueMinor:75000,costMinor:32000,profitMinor:43000,budgetHours:20}],notes:['Fixed contract snapshot.']})};
  const app=await mount(ProfitabilityPage,{state:state(),mutate(){}});
  try {await click(button(app.container,'Calculate financials'));assert.match(app.container.textContent,/\$750\.00/);assert.match(app.container.textContent,/\$320\.00/);assert.match(app.container.textContent,/\$430\.00/);assert.match(app.container.textContent,/57.3% margin/);assert.match(app.container.textContent,/8h \/ 20h/);assert.match(app.container.textContent,/Fixed contract snapshot/);} finally {delete window.rize;await app.cleanup();}
});
