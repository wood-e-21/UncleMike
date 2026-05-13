const { contextBridge, ipcRenderer } = require("electron");

contextBridge.exposeInMainWorld("mike", {
  // Lock-screen flow
  getState: () => ipcRenderer.invoke("mike:getState"),
  pickWorkspace: () => ipcRenderer.invoke("mike:pickWorkspace"),
  setPin: (pin) => ipcRenderer.invoke("mike:setPin", pin),
  unlock: (pin) => ipcRenderer.invoke("mike:unlock", pin),

  // Post-unlock — used by the supabase shim and any code needing the API URL
  getToken: () => ipcRenderer.invoke("mike:getToken"),
  getUser: () => ipcRenderer.invoke("mike:getUser"),
  getApiBase: () => ipcRenderer.invoke("mike:getApiBase"),
  signOut: () => ipcRenderer.invoke("mike:signOut"),
  changePin: (oldPin, newPin) =>
    ipcRenderer.invoke("mike:changePin", oldPin, newPin),
});
