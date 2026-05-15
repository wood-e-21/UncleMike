const $ = (id) => document.getElementById(id);

function setError(msg) {
  $("error").textContent = msg ?? "";
}

function show(sectionId, subtitle) {
  for (const id of ["workspaceSection", "setPasswordSection", "unlockSection"]) {
    $(id).hidden = id !== sectionId;
  }
  $("subtitle").textContent = subtitle;
}

async function refresh() {
  const state = await window.mike.getState();
  $("card").hidden = false;
  setError("");

  if (!state.workspace) {
    show("workspaceSection", "Set up your workspace");
    $("workspacePath").textContent = "";
  } else if (!state.hasPassword) {
    show("setPasswordSection", "Create a PIN");
    $("workspacePath").textContent = state.workspace;
  } else {
    show("unlockSection", "Welcome back");
    $("workspaceLabel").textContent = `Workspace: ${state.workspace}`;
    $("password").focus();
  }
}

$("pickBtn").addEventListener("click", async () => {
  setError("");
  const result = await window.mike.pickWorkspace();
  if (!result.ok) return;
  await refresh();
});

$("setPasswordBtn").addEventListener("click", async () => {
  setError("");
  const pw = $("newPassword").value;
  const confirm = $("newPasswordConfirm").value;
  if (!/^\d{4,8}$/.test(pw)) {
    setError("PIN must be 4-8 digits.");
    return;
  }
  if (pw !== confirm) {
    setError("PINs do not match.");
    return;
  }
  const result = await window.mike.setPin(pw);
  if (!result.ok) {
    setError(result.error ?? "Failed to set PIN.");
    return;
  }
  $("newPassword").value = "";
  $("newPasswordConfirm").value = "";
  await refresh();
});

function setBusy(busy, message) {
  $("status").hidden = !busy;
  $("statusText").textContent = message ?? "Starting Mike…";
  $("unlockBtn").disabled = busy;
  $("password").disabled = busy;
  $("setPasswordBtn").disabled = busy;
}

$("unlockBtn").addEventListener("click", async () => {
  setError("");
  const pw = $("password").value;
  setBusy(true, "Unlocking…");
  const result = await window.mike.unlock(pw);
  if (!result.ok) {
    setBusy(false);
    setError(result.error ?? "Failed to unlock.");
    $("password").value = "";
    $("password").focus();
    return;
  }
  // Success path: main process is loading the app. Keep the spinner so the
  // window doesn't look frozen while the backend warms up.
  setBusy(true, "Starting your workspace…");
  // On success, main process navigates the window — nothing more to do here.
});

$("password").addEventListener("keydown", (e) => {
  if (e.key === "Enter") $("unlockBtn").click();
});
$("newPasswordConfirm").addEventListener("keydown", (e) => {
  if (e.key === "Enter") $("setPasswordBtn").click();
});

refresh().catch((err) => setError(String(err)));
