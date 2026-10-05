(() => {
  const el = id => document.getElementById(`console-${id}`);
  let socket;
  let paused = false;
  let rows = [];
  const access = window.viewerAccess;

  function setStatus(message, kind = "idle") {
    const panel = el("status");
    const copy = panel?.querySelector("span:last-child");
    if (copy) copy.textContent = message;
    else if (panel) panel.textContent = message;
    panel?.classList.toggle("is-online", kind === "online");
    panel?.classList.toggle("is-error", kind === "error");
    const badge = el("connection-badge");
    if (!badge) return;
    badge.className = `status-badge status-${kind === "online" ? "online" : kind === "error" ? "failed" : "idle"}`;
    badge.textContent = !access.get() ? "Access required" : kind === "online" ? "Streaming" : kind === "error" ? "Stream issue" : "Stream stopped";
  }

  function setDeviceOptions(items) {
    const select = el("device");
    const previous = select.value;
    if (!items.length) {
      select.replaceChildren(new Option("No registered devices", ""));
      select.disabled = true;
      el("connect").disabled = true;
      setStatus("No devices are registered. Flash and start the managed OTA firmware first.", "error");
      return;
    }
    select.replaceChildren(...items.map(device => new Option(`${device.name} (${device.device_id})`, device.device_id)));
    if (items.some(device => device.device_id === previous)) select.value = previous;
    select.disabled = false;
    el("connect").disabled = !access.get() || !!socket;
    setStatus(`${items.length} device${items.length === 1 ? "" : "s"} found.`);
  }

  async function devices() {
    const token = access.get();
    if (!token) return;
    const refresh = el("refresh");
    refresh.disabled = true;
    refresh.textContent = "Loading...";
    setStatus("Loading registered devices...");
    try {
      const items = await otaRequest("/api/ota/console/devices", {
        headers: { Authorization: `Bearer ${token}` },
      });
      if (access.get() !== token) return;
      setDeviceOptions(items);
    } catch (error) {
      if (access.get() !== token) return;
      el("device").replaceChildren(new Option("Devices unavailable", ""));
      el("device").disabled = true;
      el("connect").disabled = true;
      setStatus(error.message, "error");
    } finally {
      refresh.disabled = !access.get();
      refresh.innerHTML = '<span aria-hidden="true">&#8635;</span> Refresh';
    }
  }

  function render() {
    if (paused) return;
    const filter = el("filter").value.toLowerCase();
    const level = el("level").value;
    const visible = rows.filter(row =>
      (level === "ALL" || row.level === level) && String(row.message || "").toLowerCase().includes(filter)
    );
    el("output").textContent = visible.map(row =>
      `[${row.received_at || ""}] ${row.level || "NOTICE"} ${row.target || ""}: ${row.message}${row.dropped ? ` [device dropped ${row.dropped} logs]` : ""}`
    ).join("\n");
    el("empty").classList.toggle("hidden", rows.length > 0);
    el("download").disabled = rows.length === 0;
    if (el("scroll").checked) el("output").scrollTop = el("output").scrollHeight;
  }

  function disconnect(message = "Disconnected.") {
    const active = socket;
    socket = undefined;
    active?.close();
    el("disconnect").disabled = true;
    el("connect").disabled = !el("device").value || !access.get();
    setStatus(message);
  }

  el("connect").onclick = () => {
    const token = access.get();
    if (!el("device").value || !token) {
      setStatus("Select a registered device and enter the access token.", "error");
      return;
    }
    socket?.close();
    rows = [];
    render();
    const url = new URL("/api/ota/console/ws", OTA_API_BASE);
    url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
    const active = socket = new WebSocket(url);
    el("connect").disabled = true;
    setStatus("Opening a secure log stream...");
    active.onopen = () => {
      active.send(JSON.stringify({ token, device_id: el("device").value }));
      setStatus("Authenticating console access...");
    };
    active.onmessage = event => {
      if (socket !== active) return;
      try {
        const row = JSON.parse(event.data);
        setStatus(`Receiving application logs from ${el("device").value}.`, "online");
        el("disconnect").disabled = false;
        rows.push(row.notice ? { message: row.notice, level: "WARN" } : row);
        if (rows.length > 2000) rows.shift();
        render();
      } catch {
        setStatus("The server sent an invalid log message.", "error");
      }
    };
    active.onclose = () => {
      if (socket === active) disconnect("Stream closed. Check the token, MQTT service, and device connection.");
    };
    active.onerror = () => {
      if (socket === active) setStatus("Connection failed. Check the server configuration and access token.", "error");
    };
  };

  el("disconnect").onclick = () => {
    disconnect();
  };
  el("refresh").onclick = devices;
  el("device").onchange = () => {
    if (socket) disconnect("Device changed. Connect when ready.");
    rows = [];
    el("connect").disabled = !el("device").value || !access.get();
    render();
  };
  el("pause").onclick = () => {
    paused = !paused;
    el("pause").textContent = paused ? "Resume" : "Pause";
    if (!paused) render();
  };
  el("filter").oninput = render;
  el("level").onchange = render;
  el("download").onclick = () => {
    if (!rows.length) return;
    const url = URL.createObjectURL(new Blob(rows.map(row => `${JSON.stringify(row)}\n`), { type: "application/x-ndjson" }));
    const anchor = document.createElement("a");
    anchor.href = url;
    anchor.download = `${el("device").value || "device"}-logs.ndjson`;
    anchor.click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  };
  access.subscribe(() => {
    const connected = !!access.get();
    for (const id of ["refresh", "level", "filter", "scroll", "pause"]) el(id).disabled = !connected;
    if (!connected) {
      disconnect("Connect viewer access to choose a device.");
      el("device").replaceChildren(new Option("Connect viewer access first", ""));
      el("device").disabled = true;
      el("connect").disabled = true;
      paused = false;
      el("pause").textContent = "Pause";
      rows = [];
      render();
    } else if (!document.getElementById("console-section").classList.contains("hidden")) devices();
  });
  document.getElementById("nav-console").onclick = async event => {
    event.preventDefault();
    showSection("console-section", "console");
    if (access.get()) await devices();
  };
  window.addEventListener("beforeunload", () => socket?.close());
})();
