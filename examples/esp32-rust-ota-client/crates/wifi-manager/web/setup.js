(() => {
  "use strict";
  const $ = id => document.getElementById(id);
  const isSetup = document.body.dataset.page === "setup";
  let state = null, posting = false, edited = false, wasConnected = false, online = false;
  const text = (id, value) => { if ($(id)) $(id).textContent = value; };
  const notice = (id, value) => { if ($(id)) { text(id, value); $(id).hidden = !value; } };

  async function request(path, body) {
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), 7000);
    try {
      const response = await fetch(path, {
        method: body === undefined ? "GET" : "POST", cache: "no-store", credentials: "omit",
        signal: controller.signal,
        headers: body === undefined ? {} : { "Content-Type": "application/json", "X-Setup-Token": state?.csrf || "" },
        ...(body === undefined ? {} : { body: JSON.stringify(body) })
      });
      const data = await response.json();
      if (!response.ok) throw new Error(data.error || "Request failed. Please retry.");
      return data;
    } finally { clearTimeout(timer); }
  }
  function validForm() {
    const ssid = $("ssid").value, password = $("password").value;
    const bytes = value => new TextEncoder().encode(value).length;
    return bytes(ssid) > 0 && bytes(ssid) <= 32 && !ssid.includes("\0")
      && ($("open-network").checked || (!password.includes("\0")
        && ((bytes(password) >= 8 && bytes(password) <= 63) || /^[0-9a-f]{64}$/i.test(password))));
  }
  function controls() {
    const busy = posting || !online || !state || state.busy || state.scanning;
    if (isSetup) {
      $("connect").disabled = busy || !validForm();
      $("scan").disabled = busy;
      $("password").disabled = $("open-network").checked || posting;
      $("show-password").disabled = $("open-network").checked;
    }
    $("retry").disabled = busy;
  }
  function showNetworks(networks) {
    const signature = JSON.stringify(networks);
    if ($("networks").dataset.signature === signature) return;
    $("networks").dataset.signature = signature;
    $("networks").replaceChildren();
    for (const network of networks) {
      const button = document.createElement("button");
      button.type = "button"; button.className = "network";
      button.dataset.ssid = network.ssid;
      button.setAttribute("aria-pressed", String($("ssid").value === network.ssid));
      const name = document.createElement("span");
      name.className = "network-name"; name.textContent = network.ssid;
      const detail = document.createElement("span");
      detail.className = "network-detail";
      detail.textContent = (network.secured ? "Secured" : "Open") + " · " + (network.rssi > -65 ? "Strong" : network.rssi > -80 ? "Fair" : "Weak");
      button.append(name, detail);
      button.addEventListener("click", () => {
        edited = true; $("ssid").value = network.ssid; $("password").value = "";
        $("open-network").checked = !network.secured;
        document.querySelectorAll(".network").forEach(item => item.setAttribute("aria-pressed", String(item === button)));
        controls(); (network.secured ? $("password") : $("ssid")).focus();
      });
      $("networks").append(button);
    }
  }
  function render() {
    text("device-name", state.ap_name);
    $("retry").hidden = !state.saved_ssid;
    if (isSetup) {
      if (!edited && state.saved_ssid) $("ssid").value = state.saved_ssid;
      showNetworks(state.networks || []);
      text("scan-message", state.scanning ? "Scanning…" : state.scan_error || (state.networks.length ? "Select a network, or enter its name below." : "Nothing found. Refresh or enter a network name below."));
      text("saved-network", state.saved_ssid ? "Saved: " + state.saved_ssid : "No saved network yet.");
      if (state.phase === "connected") notice("connection-warning", "Board connected. This setup hotspot will close shortly.");
    } else {
      document.querySelector(".status-card").dataset.phase = state.phase;
      text("status-title", { connecting: "Connecting to your Wi-Fi…", connected: "You're connected.", failed: "Let's try that again.", idle: "Ready to connect." }[state.phase] || "Checking your board…");
      text("status-message", state.message);
      text("status-network", state.phase === "connected" ? state.saved_ssid || "—" : state.target_ssid || state.saved_ssid || "Not selected");
      text("status-ip", state.ip_address || "Not connected");
      notice("closing-message", state.phase === "connected"
        ? "Wi-Fi is saved. The setup hotspot closes in " + (state.close_in ?? 20) + " seconds. Rejoin your usual Wi-Fi to open the dashboard. Internet, OTA and MQTT are checked separately." : "");
      text("edit-network", state.phase === "failed" ? "Check password or choose another network" : "Choose or change Wi-Fi");
    }
    wasConnected ||= state.phase === "connected";
    controls();
  }
  async function poll() {
    try {
      state = await request("/api/status"); online = true;
      notice("connection-warning", ""); render();
    } catch (_) {
      online = false;
      notice("connection-warning", wasConnected
        ? "Setup connection closed. If Wi-Fi connected successfully, rejoin your usual network. If the setup hotspot returns, reconnect to it."
        : "Cannot reach the board. Stay on its setup Wi-Fi and open http://192.168.4.1. Retrying…");
      controls();
    }
    setTimeout(poll, 2000);
  }
  async function action(path, body, navigate) {
    if (posting) return;
    posting = true; controls();
    const errorId = isSetup ? "form-error" : "status-error";
    notice(errorId, "");
    try {
      await request(path, body);
      if (isSetup && path === "/api/connect") $("password").value = "";
      if (navigate) { location.assign("/status"); return; }
      state = await request("/api/status"); render();
    } catch (error) { notice(errorId, error.message); }
    finally { posting = false; controls(); }
  }
  $("retry").addEventListener("click", () => action("/api/retry", {}, true));
  if (isSetup) {
    $("ssid").addEventListener("input", () => { edited = true; controls(); });
    $("password").addEventListener("input", controls);
    $("open-network").addEventListener("change", () => {
      $("password").value = ""; controls();
    });
    $("show-password").addEventListener("click", () => {
      const show = $("password").type === "password";
      $("password").type = show ? "text" : "password";
      $("show-password").setAttribute("aria-pressed", String(show));
      text("show-password", show ? "Hide password" : "Show password");
    });
    $("scan").addEventListener("click", () => action("/api/scan", {}, false));
    $("wifi-form").addEventListener("submit", event => {
      event.preventDefault();
      if (!validForm() || $("connect").disabled) return;
      action("/api/connect", { ssid: $("ssid").value, password: $("open-network").checked ? "" : $("password").value }, true);
    });
  }
  poll();
})();
