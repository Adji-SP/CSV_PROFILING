(() => {
  const el = id => document.getElementById('console-' + id);
  let socket, paused = false, rows = [];
  const status = text => { el('status').textContent = text; };
  async function devices() {
    try {
      if (!el('token').value) { status('Enter your console token, then refresh devices.'); return; }
      const items = await otaRequest('/api/ota/console/devices', {
        headers: {Authorization: 'Bearer ' + el('token').value}
      });
      el('device').replaceChildren(...items.map(d => {
        const o = document.createElement('option');
        o.value = d.device_id; o.textContent = d.name + ' (' + d.device_id + ')'; return o;
      }));
    } catch (e) { status(e.message); }
  }
  function render() {
    if (paused) return;
    const filter = el('filter').value.toLowerCase(), level = el('level').value;
    el('output').textContent = rows.filter(r => (level === 'ALL' || r.level === level) &&
      r.message.toLowerCase().includes(filter)).map(r =>
      `[${r.received_at || ''}] ${r.level || 'NOTICE'} ${r.target || ''}: ${r.message}${r.dropped ? ' [device dropped ' + r.dropped + ' logs]' : ''}`).join('\n');
    if (el('scroll').checked) el('output').scrollTop = el('output').scrollHeight;
  }
  el('connect').onclick = () => {
    if (!el('device').value || !el('token').value) { status('Select a device and enter the access token.'); return; }
    socket?.close(); rows = []; render();
    const url = new URL('/api/ota/console/ws', OTA_API_BASE);
    url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:';
    const active = socket = new WebSocket(url);
    status('Connecting…');
    active.onopen = () => {
      active.send(JSON.stringify({ token: el('token').value, device_id: el('device').value }));
      status('Authenticating…');
    };
    active.onmessage = e => {
      if (socket !== active) return;
      try {
        const row = JSON.parse(e.data);
        status('Receiving logs');
        rows.push(row.notice ? {message:row.notice, level:'WARN'} : row);
        if (rows.length > 2000) rows.shift();
        render();
      } catch { status('Invalid log message received'); }
    };
    active.onclose = () => { if(socket === active) status('Disconnected. Check token/device, then reconnect.'); };
    active.onerror = () => { if(socket === active) status('Connection failed. Check service and console configuration.'); };
  };
  el('disconnect').onclick = () => socket?.close();
  el('refresh').onclick = devices;
  el('device').onchange = () => { socket?.close(); rows=[]; render(); };
  el('pause').onclick = () => { paused=!paused; el('pause').textContent=paused?'Resume':'Pause'; render(); };
  el('filter').oninput = render;
  el('level').onchange = render;
  el('download').onclick = () => {
    const url=URL.createObjectURL(new Blob(rows.map(r=>JSON.stringify(r)+'\n'),{type:'application/x-ndjson'}));
    const a=document.createElement('a'); a.href=url; a.download='device-logs.ndjson'; a.click();
    setTimeout(()=>URL.revokeObjectURL(url),1000);
  };
  document.getElementById('nav-console').onclick = e => { e.preventDefault(); showSection('console-section','console'); devices(); };
  window.addEventListener('beforeunload',()=>socket?.close());
})();
