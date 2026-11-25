'use strict';
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = (s) => document.querySelector(s);
const GB = 1073741824;

// Same formatting as the reference page.
const fb = (b) => {
  const u = ['B', 'KB', 'MB', 'GB'];
  let i = 0;
  while (b >= 1000 && i < 3) { b /= 1024; i++; }
  return (b >= 100 || i === 0 ? Math.round(b) : b.toFixed(1)) + ' ' + u[i];
};
const hue = (n) => {
  let h = 0;
  for (const c of n) h = (h * 31 + c.charCodeAt(0)) % 360;
  return `hsl(${h} 45% 45%)`;
};
const set = (id, t) => { const e = $(id); if (e.textContent !== t) e.textContent = t; };

// The logo, drawn the way the reference page draws it.
(() => {
  const x = $('#logo').getContext('2d');
  x.scale(44 / 256, 44 / 256);
  x.fillStyle = '#0d0f0e';
  x.beginPath(); x.roundRect(16, 16, 224, 224, 52); x.fill();
  x.strokeStyle = '#b8f5d0'; x.lineWidth = 18; x.lineCap = 'round'; x.lineJoin = 'round';
  x.beginPath();
  x.moveTo(50, 140); x.lineTo(92, 140); x.lineTo(112, 88); x.lineTo(140, 182);
  x.lineTo(160, 120); x.lineTo(174, 140); x.lineTo(206, 140);
  x.stroke();
})();

// A 90-sample sparkline, values 0 to 100.
function spark(c, v) {
  const r = c.getBoundingClientRect(), d = devicePixelRatio || 1;
  const W = Math.max(1, r.width), H = Math.max(1, r.height);
  c.width = Math.round(W * d); c.height = Math.round(H * d);
  const x = c.getContext('2d'); x.scale(d, d);
  const L = 90, st = W / (L - 1), o = L - v.length, col = getComputedStyle(c).color;
  x.beginPath();
  v.forEach((y, i) => {
    const X = (o + i) * st, Y = H - 2 - Math.min(1, y / 100) * (H - 5);
    if (i) x.lineTo(X, Y); else x.moveTo(X, Y);
  });
  x.strokeStyle = col; x.lineWidth = 2; x.lineJoin = 'round'; x.stroke();
  x.lineTo((L - 1) * st, H); x.lineTo(o * st, H); x.closePath();
  x.globalAlpha = 0.16; x.fillStyle = col; x.fill();
}

function uptime(s) {
  const d = Math.floor(s / 86400), h = Math.floor((s % 86400) / 3600), m = Math.floor((s % 3600) / 60);
  return d ? `${d}d ${h}h` : h ? `${h}h ${m}m` : `${m}m`;
}

function rows(apps) {
  const max = Math.max(1, ...apps.map((a) => a.mem_bytes));
  const items = apps.slice(0, 6).map((a) => {
    const li = document.createElement('li');
    const ic = document.createElement('span'); ic.className = 'ic';
    const url = a.path && icons.get(a.path);
    if (url) {
      // The app's own icon, 24 px in the 28 px slot.
      const img = document.createElement('img');
      img.src = url; img.alt = ''; img.width = 24; img.height = 24;
      ic.classList.add('real'); ic.append(img);
    } else {
      ic.style.background = hue(a.exe.replace(/\.exe$/i, '')); ic.textContent = (a.name[0] || '?').toUpperCase();
    }
    const nm = document.createElement('span'); nm.className = 'nm'; nm.textContent = a.name;
    const val = document.createElement('span'); val.className = 'val'; val.textContent = fb(a.mem_bytes);
    const sb = document.createElement('span'); sb.className = 'sb';
    const t = document.createElement('span');
    t.textContent = a.exe + (a.processes > 1 ? ' · ' + a.processes + ' processes' : '');
    const br = document.createElement('span'); br.className = 'br';
    const i = document.createElement('i'); i.style.width = (a.mem_bytes / max * 100).toFixed(1) + '%';
    br.append(i); sb.append(t, br);
    li.append(ic, nm, val, sb);
    return li;
  });
  $('#list').replaceChildren(...items);
}

const icons = new Map(); // exe path -> PNG data URL, each sent once by the app
let sample = false, shownAt = 0, hist = { cpu: [], mem: [] };

function render(p) {
  const s = p.snapshot;
  hist = { cpu: p.cpu, mem: p.mem };
  const memPct = s.mem_total ? s.mem_used / s.mem_total * 100 : 0;
  set('#cpu', Math.round(s.cpu_pct) + '%');
  set('#cpuS', `${s.cores} cores · up ${uptime(s.uptime_s)}`);
  set('#memV', (s.mem_used / GB).toFixed(1) + ' GB');
  set('#memS', `of ${Math.round(s.mem_total / GB)} GB · ${Math.round(memPct)}%`);
  const hot = s.cpu_pct > 85 || memPct > 90;
  set('#status', sample ? 'Sample data, not this PC' : 'Running smoothly');
  $('#status').classList.toggle('hot', hot && !sample);
  rows(s.apps);
  spark($('#cpuC'), hist.cpu);
  spark($('#memC'), hist.mem);
  shownAt = performance.now();
  set('#ago', '2s ago');
}

// First frame with data painted: tell the app, once per show.
function reportPainted() {
  requestAnimationFrame(() => setTimeout(() => invoke('frame_painted'), 0));
}

setInterval(() => {
  if (!shownAt) return;
  const n = Math.round((performance.now() - shownAt) / 1000);
  // Snapshots come every 2 s, so a fresh one reads "2s ago" like the reference.
  set('#ago', Math.max(2, n) + 's ago');
}, 1000);

$('#close').addEventListener('click', () => invoke('hide_panel'));
addEventListener('keydown', (e) => { if (e.key === 'Escape') invoke('hide_panel'); });
addEventListener('contextmenu', (e) => e.preventDefault());

(async () => {
  const cfg = await invoke('config');
  sample = cfg.sample;
  if (cfg.theme) document.documentElement.dataset.theme = cfg.theme;
  await listen('icons', (e) => {
    for (const [path, url] of Object.entries(e.payload)) icons.set(path, url);
  });
  await listen('snapshot', (e) => {
    render(e.payload);
    if (e.payload.first) reportPainted();
  });
  await invoke('ready');
})();
