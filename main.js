const { app, BrowserWindow, ipcMain, shell } = require('electron');
const path = require('path');
const fs = require('fs');
const pty = require('@lydell/node-pty');

const BASH_CANDIDATES = [
  'C:\\Program Files\\Git\\bin\\bash.exe',
  'C:\\Program Files (x86)\\Git\\bin\\bash.exe',
  path.join(process.env.LOCALAPPDATA || '', 'Programs', 'Git', 'bin', 'bash.exe'),
];
const BASH = BASH_CANDIDATES.find((p) => fs.existsSync(p));
const ICON = path.join(__dirname, 'icon.ico');
const RC = (app.isPackaged ? path.join(process.resourcesPath, 'fast-rc.sh') : path.join(__dirname, 'fast-rc.sh'))
  .replace(/\\/g, '/');

const ptys = new Map();
let nextId = 1;
let win;

function createWindow() {
  win = new BrowserWindow({
    width: 1100,
    height: 700,
    backgroundColor: '#1e1e1e',
    title: 'Git Bash',
    icon: fs.existsSync(ICON) ? ICON : undefined,
    autoHideMenuBar: true,
    webPreferences: {
      preload: path.join(__dirname, 'preload.js'),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: false,
    },
  });
  win.setMenuBarVisibility(false);
  win.loadFile('index.html');
  win.on('closed', () => {
    for (const p of ptys.values()) p.kill();
    ptys.clear();
  });
}

ipcMain.handle('pty:spawn', (_e, { cols, rows }) => {
  if (!BASH) throw new Error('Git Bash(bash.exe)를 찾을 수 없습니다.');
  const id = nextId++;
  const p = pty.spawn(BASH, ['--noprofile', '--rcfile', RC, '-i'], {
    name: 'xterm-256color',
    cols,
    rows,
    cwd: process.env.USERPROFILE || process.cwd(),
    env: { ...process.env, TERM: 'xterm-256color', CHERE_INVOKING: '1' },
    useConpty: true,
  });
  ptys.set(id, p);
  p.onData((data) => win && !win.isDestroyed() && win.webContents.send('pty:data', id, data));
  p.onExit(() => {
    ptys.delete(id);
    if (win && !win.isDestroyed()) win.webContents.send('pty:exit', id);
  });
  return id;
});

ipcMain.on('pty:write', (_e, id, data) => ptys.get(id)?.write(data));
ipcMain.on('pty:resize', (_e, id, cols, rows) => {
  try { ptys.get(id)?.resize(cols, rows); } catch {}
});
ipcMain.on('pty:kill', (_e, id) => {
  ptys.get(id)?.kill();
  ptys.delete(id);
});
ipcMain.on('app:quit', () => win?.close());
ipcMain.on('open:url', (_e, url) => {
  if (/^https?:\/\//.test(url)) shell.openExternal(url);
});

app.whenReady().then(createWindow);
app.on('window-all-closed', () => app.quit());
