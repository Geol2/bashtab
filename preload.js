const { contextBridge, ipcRenderer, clipboard } = require('electron');

contextBridge.exposeInMainWorld('api', {
  spawn: (cols, rows) => ipcRenderer.invoke('pty:spawn', { cols, rows }),
  write: (id, data) => ipcRenderer.send('pty:write', id, data),
  resize: (id, cols, rows) => ipcRenderer.send('pty:resize', id, cols, rows),
  kill: (id) => ipcRenderer.send('pty:kill', id),
  quit: () => ipcRenderer.send('app:quit'),
  openUrl: (url) => ipcRenderer.send('open:url', url),
  onData: (cb) => ipcRenderer.on('pty:data', (_e, id, data) => cb(id, data)),
  onExit: (cb) => ipcRenderer.on('pty:exit', (_e, id) => cb(id)),
  readClipboard: () => clipboard.readText(),
  writeClipboard: (text) => clipboard.writeText(text),
});
