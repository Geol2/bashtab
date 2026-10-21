const tabsEl = document.getElementById('tabs');
const termsEl = document.getElementById('terminals');
const sessions = new Map(); // ptyId -> { term, fit, tabEl, paneEl }
let activeId = null;

const THEME = {
  background: '#1e1e1e',
  foreground: '#cccccc',
  cursor: '#ffffff',
  selectionBackground: '#264f78',
  black: '#000000', red: '#cd3131', green: '#0dbc79', yellow: '#e5e510',
  blue: '#2472c8', magenta: '#bc3fbc', cyan: '#11a8cd', white: '#e5e5e5',
  brightBlack: '#666666', brightRed: '#f14c4c', brightGreen: '#23d18b', brightYellow: '#f5f543',
  brightBlue: '#3b8eea', brightMagenta: '#d670d6', brightCyan: '#29b8db', brightWhite: '#e5e5e5',
};

async function newTab() {
  const paneEl = document.createElement('div');
  paneEl.className = 'term';
  termsEl.appendChild(paneEl);

  const term = new Terminal({
    fontFamily: "'Cascadia Mono', Consolas, 'D2Coding', 'Malgun Gothic', monospace",
    fontSize: 14,
    cursorBlink: true,
    scrollback: 10000,
    theme: THEME,
    allowProposedApi: true,
  });
  const fit = new FitAddon.FitAddon();
  term.loadAddon(fit);
  term.loadAddon(new WebLinksAddon.WebLinksAddon((_e, url) => api.openUrl(url)));
  term.open(paneEl);

  paneEl.classList.add('active');
  fit.fit();

  const id = await api.spawn(term.cols, term.rows);

  const tabEl = document.createElement('div');
  tabEl.className = 'tab';
  tabEl.innerHTML = '<span class="title">Git Bash</span><button class="close" title="닫기">×</button>';
  tabEl.addEventListener('mousedown', (e) => {
    if (e.button === 1) { e.preventDefault(); closeTab(id); }
  });
  tabEl.addEventListener('click', (e) => {
    if (e.target.classList.contains('close')) closeTab(id);
    else activate(id);
  });
  tabsEl.appendChild(tabEl);

  term.onData((data) => api.write(id, data));
  term.onResize(({ cols, rows }) => api.resize(id, cols, rows));
  term.onTitleChange((title) => {
    tabEl.querySelector('.title').textContent = title || 'Git Bash';
    if (id === activeId) document.title = title || 'Git Bash';
  });
  term.attachCustomKeyEventHandler((e) => handleKey(e, id));
  term.element.addEventListener('contextmenu', (e) => {
    e.preventDefault();
    if (term.hasSelection()) {
      api.writeClipboard(term.getSelection());
      term.clearSelection();
    } else {
      api.write(id, api.readClipboard());
    }
  });

  sessions.set(id, { term, fit, tabEl, paneEl });
  activate(id);
}

function activate(id) {
  const s = sessions.get(id);
  if (!s) return;
  for (const [otherId, o] of sessions) {
    const on = otherId === id;
    o.tabEl.classList.toggle('active', on);
    o.paneEl.classList.toggle('active', on);
  }
  activeId = id;
  s.tabEl.scrollIntoView({ block: 'nearest', inline: 'nearest' });
  document.title = s.tabEl.querySelector('.title').textContent;
  requestAnimationFrame(() => { s.fit.fit(); s.term.focus(); });
}

function closeTab(id) {
  api.kill(id);
  removeTab(id);
}

function removeTab(id) {
  const s = sessions.get(id);
  if (!s) return;
  const ids = [...sessions.keys()];
  const idx = ids.indexOf(id);
  s.term.dispose();
  s.tabEl.remove();
  s.paneEl.remove();
  sessions.delete(id);
  if (sessions.size === 0) return api.quit();
  if (activeId === id) {
    const rest = [...sessions.keys()];
    activate(rest[Math.min(idx, rest.length - 1)]);
  }
}

function cycle(delta) {
  const ids = [...sessions.keys()];
  const i = ids.indexOf(activeId);
  activate(ids[(i + delta + ids.length) % ids.length]);
}

function handleKey(e, id) {
  if (e.type !== 'keydown') return true;
  const key = e.key.toLowerCase();
  const s = sessions.get(id);

  if (e.ctrlKey && e.shiftKey) {
    if (key === 't') { newTab(); return false; }
    if (key === 'w') { closeTab(id); return false; }
    if (key === 'c') {
      if (s.term.hasSelection()) api.writeClipboard(s.term.getSelection());
      return false;
    }
    if (key === 'v') { api.write(id, api.readClipboard()); return false; }
  }
  if (e.ctrlKey && key === 'tab') { cycle(e.shiftKey ? -1 : 1); return false; }
  if (e.ctrlKey && !e.shiftKey && key === 'v') { api.write(id, api.readClipboard()); return false; }
  if (e.altKey && /^[1-9]$/.test(e.key)) {
    const ids = [...sessions.keys()];
    const target = e.key === '9' ? ids[ids.length - 1] : ids[Number(e.key) - 1];
    if (target) activate(target);
    return false;
  }
  return true;
}

api.onData((id, data) => sessions.get(id)?.term.write(data));
api.onExit((id) => removeTab(id));

document.getElementById('new-tab').addEventListener('click', newTab);
window.addEventListener('resize', () => sessions.get(activeId)?.fit.fit());

newTab();
