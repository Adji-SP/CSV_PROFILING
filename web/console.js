(() => {
  const el = id => document.getElementById(`console-${id}`);
  let socket;
  let paused = false;
  let rows = [];

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
    badge.textContent = kind === "online" ? "Connected" : kind === "error" ? "Connection issue" : "Disconnected";
  }

  function setDeviceOptions(items) {
    const select = el("device");
    if (!items.length) {
      select.replaceChildren(new Option("No registered devices", ""));
      select.disabled = true;
      el("connect").disabled = true;
      setStatus("No devices are registered. Flash and start the managed OTA firmware first.", "error");
      return;
    }
    select.replaceChildren(...items.map(device => new Option(`${device.name} (${device.device_id})`, device.device_id)));
    select.disabled = false;
    el("connect").disabled = !el("token").value.trim();
    setStatus(`${items.length} device${items.length === 1 ? "" : "s"} found.`);
  }

  async function devices() {
    const refresh = el("refresh");
    refresh.disabled = true;
    refresh.textContent = "Loading...";
    setStatus("Loading registered devices...");
    try {
      let items;
      try {
        items = await getDevices();
      } catch (publicError) {
        const token = el("token").value.trim();
        if (!token) throw new Error("Enter the console access token to load devices from this server.");
        items = await otaRequest("/api/ota/console/devices", {
          headers: { Authorization: `Bearer ${token}` },
        });
      }
      setDeviceOptions(items);
    } catch (error) {
      el("device").replaceChildren(new Option("Devices unavailable", ""));
      el("device").disabled = true;
      el("connect").disabled = true;
      setStatus(error.message, "error");
    } finally {
      refresh.disabled = false;
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
    el("connect").disabled = !el("device").value || !el("token").value.trim();
    setStatus(message);
  }

  el("connect").onclick = () => {
    const token = el("token").value.trim();
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

  el("disconnect").onclick = () => disconnect();
  el("refresh").onclick = devices;
  el("token").oninput = () => { el("connect").disabled = !el("device").value || !el("token").value.trim(); };
  el("device").onchange = () => {
    if (socket) disconnect("Device changed. Connect when ready.");
    rows = [];
    el("connect").disabled = !el("device").value || !el("token").value.trim();
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
  document.getElementById("nav-console").onclick = event => {
    event.preventDefault();
    showSection("console-section", "console");
    devices();
  };
  window.addEventListener("beforeunload", () => socket?.close());
})();
