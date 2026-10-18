export const clamp=(n,a,b)=>Math.min(b,Math.max(a,n));
export const dayStart=d=>{const v=new Date(d);v.setHours(0,0,0,0);return v;};
export const shiftDay=(d,n)=>{const v=new Date(d);v.setDate(v.getDate()+n);return v;};
export function shiftMonth(d,n){const v=new Date(d),day=v.getDate();v.setDate(1);v.setMonth(v.getMonth()+n);const last=new Date(v.getFullYear(),v.getMonth()+1,0).getDate();v.setDate(Math.min(day,last));return v;}
export const sameDay=(a,b)=>dayStart(a).getTime()===dayStart(b).getTime();
export function clock(ms){let s=Math.max(0,Math.floor(ms/1000));return `${Math.floor(s/3600)}:${String(Math.floor(s/60)%60).padStart(2,'0')}:${String(s%60).padStart(2,'0')}`;}
export function duration(ms){const m=Math.round(Math.max(0,ms)/60000);return m>=60?`${Math.floor(m/60)}h ${m%60}m`:`${m}m`;}
export const timerElapsed=(t,now=Date.now())=>t?Math.max(0,t.elapsedMs||0)+(t.status==='running'&&t.resumedAt?Math.max(0,now-Date.parse(t.resumedAt)):0):0;
export function dayEntries(entries,date){const a=+dayStart(date),b=+shiftDay(dayStart(date),1);return entries.filter(e=>Date.parse(e.end)>a&&Date.parse(e.start)<b).map(e=>({...e,visibleStart:Math.max(a,Date.parse(e.start)),visibleEnd:Math.min(b,Date.parse(e.end))}));}
export function unionDuration(entries){const intervals=entries.map(e=>[e.visibleStart??Date.parse(e.start),e.visibleEnd??Date.parse(e.end)]).sort((a,b)=>a[0]-b[0]);let total=0,left=null,right=null;for(const [a,b]of intervals){if(left===null){left=a;right=b;}else if(a<=right)right=Math.max(right,b);else{total+=right-left;left=a;right=b;}}return total+(left===null?0:right-left);}
export function assignLanes(entries){const sorted=entries.map(e=>({...e})).sort((a,b)=>a.visibleStart-b.visibleStart),clusters=[];let end=-Infinity;for(const e of sorted){if(e.visibleStart>=end){clusters.push([]);end=e.visibleEnd;}else end=Math.max(end,e.visibleEnd);clusters.at(-1).push(e);}for(const group of clusters){const lanes=[];for(const e of group){let i=lanes.findIndex(t=>t<=e.visibleStart);if(i<0)i=lanes.length;lanes[i]=e.visibleEnd;e.lane=i;}for(const e of group)e.lanes=lanes.length;}return sorted;}
export function inputTimestamp(iso){const d=new Date(iso);return `${d.getFullYear()}-${String(d.getMonth()+1).padStart(2,'0')}-${String(d.getDate()).padStart(2,'0')}T${String(d.getHours()).padStart(2,'0')}:${String(d.getMinutes()).padStart(2,'0')}:${String(d.getSeconds()).padStart(2,'0')}.${String(d.getMilliseconds()).padStart(3,'0')}`;}
export const id=()=>crypto.randomUUID();
