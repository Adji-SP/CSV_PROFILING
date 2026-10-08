/* One authenticated, in-memory viewer session for both Monitoring views. */
(() => {
  const loginSession = !!window.APP_CONFIG?.loginSession;
  let token = loginSession ? "browser-session" : "";
  let attempt = 0;
  const listeners = new Set();
  const el = id => document.getElementById(`viewer-access-${id}`);
  function notify() { for (const listener of listeners) listener(); }
  function render(message) {
    if (loginSession) {
      el("form").closest(".viewer-access-panel")?.classList.add("hidden");
      el("form").closest(".viewer-access-panel")?.setAttribute("hidden", "");
      el("form").classList.add("hidden");
      return;
    }
    const connected = !!token;
    el("token").disabled = connected;
    el("connect").disabled = connected || !el("token").value.trim();
    el("disconnect").disabled = !connected;
    el("label").classList.toggle("hidden", connected);
    el("token").classList.toggle("hidden", connected);
    el("connect").classList.toggle("hidden", connected);
    el("form").classList.toggle("is-connected", connected);
    el("status").textContent = message || (connected ? "Connected. Reporting and Remote Console are unlocked." : "Enter your viewer token to unlock Monitoring.");
  }
  window.viewerAccess = Object.freeze({
    get: () => token,
    async connect(value) {
      const candidate = String(value || "").trim();
      if (!candidate) { render("Enter the viewer token first."); return false; }
      const current = ++attempt;
      el("connect").disabled = true;
      el("token").disabled = true;
      el("status").textContent = "Checking viewer access…";
      try {
        const response = await fetch(`${OTA_API_BASE}/api/ota/console/devices`, {
          credentials: "include",
          headers: {Authorization: `Bearer ${candidate}`},
        });
        if (current !== attempt) return false;
        if (!response.ok) {
          const body = await response.json().catch(() => ({}));
          throw new Error(response.status === 401 ? "Viewer token is incorrect." : body.error?.message || `Connection failed (${response.status}).`);
        }
        token = candidate;
        el("token").value = "";
        render();
        notify();
        return true;
      } catch (error) {
        if (current === attempt) { render(error.message); el("connect").disabled = false; }
        return false;
      }
    },
    clear() {
      if (loginSession) return;
      ++attempt;
      if (!token) { el("token").value = ""; render(); return; }
      token = "";
      el("token").value = "";
      render();
      notify();
    },
    subscribe(listener) {
      listeners.add(listener);
      if (loginSession) queueMicrotask(listener);
      return () => listeners.delete(listener);
    },
  });
  el("form").addEventListener("submit", event => {
    event.preventDefault();
    window.viewerAccess.connect(el("token").value);
  });
  el("token").addEventListener("input", () => {
    el("connect").disabled = !el("token").value.trim();
  });
  el("disconnect").addEventListener("click", () => window.viewerAccess.clear());
  render();
})();
