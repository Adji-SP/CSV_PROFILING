/* TinyML owns its state and authenticated Rust API; CSV requests are untouched. */
(() => {
  "use strict";
  const el = id => document.getElementById(`tml-${id}`);
  const esc = value => String(value ?? "—").replace(/[&<>"']/g, c => ({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"}[c]));
  const numeric = value => typeof value === "number" && Number.isFinite(value);
  const fmt = (value, digits = 2) => numeric(value) ? value.toLocaleString(undefined, {maximumFractionDigits: digits}) : "—";
  const pct = value => numeric(value) ? `${fmt(value * 100)}%` : "—";
  const time = value => value ? new Date(value).toLocaleString() : "—";
  const active = run => ["created", "running", "completing"].includes(run?.status);
  let token = "", socket, retry, timer, generation = 0, reconnectDelay = 1000;
  let selected = null, runPage = 1, resultPage = 1, chartRows = [];
  let refreshing = false, refreshAgain = false, lastMetrics = 0, reportIsFinal = false;
  function message(text) { el("message").textContent = text; }
  function connectionState(text,state="offline") {
    el("connection").className=`tml-connection is-${state}`;
    el("connection").innerHTML='<span class="tml-connection-dot"></span>'+esc(text);
  }
  function connectedControls(enabled) {
    el("token").disabled=enabled;
    el("connect-form").querySelector('button[type="submit"]').disabled=enabled;
    el("disconnect").disabled=!enabled;
    el("device").disabled=!enabled;
    el("run-status").disabled=!enabled;
    el("run-filter").querySelector("button").disabled=!enabled;
  }
  function showRunsState(total,connected=true) {
    const table=el("runs").closest(".table-scroll"),empty=el("empty"),pager=el("runs-pagination");
    const title=empty.querySelector("strong"),copy=empty.querySelector("p"),action=el("empty-connect");
    if(total>0) {table.classList.remove("hidden");empty.classList.add("hidden");pager.classList.remove("hidden");return;}
    table.classList.add("hidden");empty.classList.remove("hidden");pager.classList.add("hidden");
    empty.classList.toggle("is-compact",connected);
    title.textContent=connected ? "No inference runs yet" : "Connect to view inference runs";
    copy.textContent=connected ? "New device experiments will appear here automatically." : "Your device experiments and saved reports will appear here.";
    action.textContent=connected ? "Refresh runs" : "Enter viewer token";
  }
  async function request(path, raw = false) {
    if (!token) throw new Error("Enter the viewer token first.");
    const session = generation;
    const response = await fetch(`${OTA_API_BASE}/api/tinyml${path}`, {headers: {Authorization: `Bearer ${token}`}});
    if (session !== generation) throw new Error("Session changed.");
    if (!response.ok) {
      const error = await response.json().catch(() => ({}));
      const e = new Error(error.error?.message || `Request failed (${response.status})`); e.status = response.status; throw e;
    }
    return raw ? response : response.json();
  }
  const guarded = fn => async event => { event?.preventDefault(); try { await fn(event); } catch (error) { message(error.message); } };
  function stats(target, values) {
    el(target).innerHTML = values.map(([label, value]) => `<div class="card tml-stat"><span>${esc(label)}</span><strong>${esc(value)}</strong></div>`).join("");
  }
  function jsonDetails(title, value) { return `<details class="tml-json"><summary>${esc(title)}</summary><pre>${esc(JSON.stringify(value, null, 2))}</pre></details>`; }
  async function overview() {
    const {overview:o} = await request("/status");
    stats("overview", [["Devices online", fmt(o.connected_devices,0)],["Active runs", fmt(o.active_runs,0)],["Completed",fmt(o.completed_runs,0)],["Inferences",fmt(o.total_inferences,0)],["Labelled accuracy",pct(o.accuracy)],["Mean inference",`${fmt(o.mean_inference_ms)} ms`]]);
    const q = new URLSearchParams({page:runPage,page_size:20});
    if (el("device").value) q.set("device_id",el("device").value);
    if (el("run-status").value) q.set("status",el("run-status").value);
    const runs = await request(`/runs?${q}`);
    el("runs").innerHTML = runs.items.map(r => `<tr><td><button class="btn-ghost-sm" data-run="${esc(r.id)}">${esc(r.run_name || r.run_id)}</button><small>${esc(r.run_id)}</small></td><td>${esc(r.device_id)}</td><td>${esc(r.model?.name)}<small>${esc(r.model?.version)}</small></td><td>${esc(r.mode)}</td><td>${esc(time(r.started_at_server))}</td><td>${fmt(r.duration_seconds)} s</td><td>${fmt(r.samples,0)}</td><td>${pct(r.accuracy)}</td><td>${fmt(r.mean_inference_ms)}</td><td>${esc(r.status)}</td></tr>`).join("") || '<tr><td colspan="10" class="table-empty">No runs yet.</td></tr>';
    pagination("runs",runPage,runs.total,20);
    showRunsState(runs.total,true);
  }
  function pagination(name,page,total,size) {
    el(`${name}-page`).textContent = `Page ${page} of ${Math.max(1,Math.ceil(total/size))} · ${total} records`;
    el(`${name}-prev`).disabled = page <= 1; el(`${name}-next`).disabled = page*size >= total;
  }
  function resultQuery() {
    const q = new URLSearchParams({page:resultPage,page_size:50});
    for (const [control,key] of [["search","search"],["actual","actual_class"],["predicted","predicted_class"],["correct","correct"],["min","min_confidence"],["max","max_confidence"],["sort","sort"],["order","order"]]) if (el(control).value !== "") q.set(key,el(control).value);
    return q;
  }
  async function results(id) {
    const data = await request(`/runs/${id}/results?${resultQuery()}`); if (selected !== id) return;
    el("results").innerHTML = data.items.map(r => `<tr><td>${esc(r.sample_id)}</td><td>${esc(time(r.server_timestamp))}</td><td>${esc(r.actual_class)}</td><td>${esc(r.predicted_class)}</td><td>${pct(r.confidence)}</td><td>${r.correct === null ? "—" : r.correct ? "Yes" : "No"}</td><td>${fmt(numeric(r.inference_us) ? r.inference_us/1000 : null)}</td><td>${fmt(numeric(r.total_pipeline_us) ? r.total_pipeline_us/1000 : null)}</td><td>${fmt(r.chip_temperature_c)}</td><td>${fmt(r.free_heap_bytes,0)}</td><td>${jsonDetails("Inspect",r)}</td></tr>`).join("") || '<tr><td colspan="11" class="table-empty">No matching results.</td></tr>';
    pagination("results",resultPage,data.total,50);
  }
  const signals = new Map();
  function numericFields(value,prefix="",out=new Map(),depth=0) {
    if (numeric(value)) out.set(prefix,value);
    else if (value && typeof value === "object" && depth < 8) for (const [key,v] of Object.entries(value)) numericFields(v,`${prefix}/${key.replace(/~/g,"~0").replace(/\//g,"~1")}`,out,depth+1);
    return out;
  }
  function pointer(value,path) {
    return !path ? value : path.slice(1).split("/").reduce((v,key) => v?.[key.replace(/~1/g,"/").replace(/~0/g,"~")],value);
  }
  function chartOptions() {
    const previous = el("signal").value; signals.clear();
    for (const [name,get] of [["Inference (ms)",r => numeric(r.inference_us) ? r.inference_us/1000 : null],["Pipeline (ms)",r => numeric(r.total_pipeline_us) ? r.total_pipeline_us/1000 : null],["Confidence",r => r.confidence],["Free heap (bytes)",r => r.free_heap_bytes],["Chip temperature (°C)",r => r.chip_temperature_c]]) if (chartRows.some(r => numeric(get(r)))) signals.set(name,get);
    const custom = new Set();
    for (const row of chartRows) for (const key of numericFields(row.other).keys()) if (custom.size < 256) custom.add(key);
    for (const key of custom) signals.set(`Other ${key || "/"}`,r => pointer(r.other,key));
    el("signal").innerHTML = [...signals.keys()].map(key => `<option>${esc(key)}</option>`).join("");
    if (signals.has(previous)) el("signal").value = previous; drawChart();
  }
  function drawChart() {
    const get = signals.get(el("signal").value);
    const points = get ? chartRows.map((r,i) => ({x:i,y:get(r)})).filter(p => numeric(p.y)) : [];
    if (!points.length) {el("chart").textContent = "No numeric measurements yet.";return;}
    const min = Math.min(...points.map(p => p.y)), max = Math.max(...points.map(p => p.y)), range = max-min || Math.max(1,Math.abs(max)*0.1);
    const xy = points.map(p => [70+p.x/Math.max(1,chartRows.length-1)*800,170-(p.y-min)/range*140]);
    el("chart").innerHTML = `<svg viewBox="0 0 900 210" role="img" aria-label="${esc(el("signal").value)}"><path d="M70 20V170H880" fill="none" stroke="#cbd5e1"/><text x="4" y="30">${esc(fmt(max))}</text><text x="4" y="170">${esc(fmt(min))}</text><text x="70" y="200">Older samples</text><text x="780" y="200">Latest</text><polyline points="${xy.map(p => p.join(",")).join(" ")}" fill="none" stroke="#655cf6" stroke-width="2"/>${xy.map((p,i) => `<circle cx="${p[0]}" cy="${p[1]}" r="2.5" fill="#655cf6"><title>${esc(fmt(points[i].y,5))}</title></circle>`).join("")}</svg>`;
    el("chart").setAttribute("aria-label",`${el("signal").value}: ${points.length} measurements, minimum ${fmt(min)}, maximum ${fmt(max)}.`);
  }
  function flattenTable(title,value) {
    const rows = [];
    function walk(v,key) {
      if (v && typeof v === "object" && !Array.isArray(v)) for (const [k,item] of Object.entries(v)) walk(item,key ? `${key} / ${k}` : k);
      else rows.push(`<tr><th>${esc(key.replace(/_/g," "))}</th><td>${esc(numeric(v) ? fmt(v,4) : v === null ? "—" : typeof v === "object" ? JSON.stringify(v) : v)}</td></tr>`);
    }
    walk(value,""); return `<h4>${esc(title)}</h4><div class="table-scroll"><table class="sessions-table tml-metrics"><tbody>${rows.join("")}</tbody></table></div>`;
  }
  async function metrics(id) {
    let data, final = true;
    try {data = await request(`/runs/${id}/report`);} catch (error) {
      if (error.status !== 404) throw error;
      final = false; data = await request(`/runs/${id}/metrics`);
    }
    if (selected !== id) return;
    reportIsFinal = final; lastMetrics = Date.now(); el("export-json").disabled = !final;
    el("report-state").textContent = `${final ? "Saved" : "Live snapshot"} · ${time(data.generated_at)}`;
    let html = flattenTable("Classification (ratios 0–1)",data.classification);
    html += '<h4>Per-class metrics</h4><div class="table-scroll"><table class="sessions-table"><thead><tr><th>Class</th><th>Support</th><th>Precision</th><th>Recall</th><th>F1</th></tr></thead><tbody>' + data.class_metrics.map(c => `<tr><th>${esc(c.class)}</th><td>${fmt(c.support,0)}</td><td>${pct(c.precision)}</td><td>${pct(c.recall)}</td><td>${pct(c.f1)}</td></tr>`).join("") + '</tbody></table></div>';
    const matrix = data.confusion_matrix;
    if (matrix?.matrix && matrix.labels.length <= 20) {
      html += '<h4>Confusion matrix</h4><p class="field-help">Rows: actual · Columns: predicted</p><div class="table-scroll"><table class="sessions-table"><thead><tr><th>Actual / Predicted</th>' + matrix.labels.map(l => `<th>${esc(l)}</th>`).join("") + '</tr></thead><tbody>' + matrix.matrix.map((row,i) => `<tr><th>${esc(matrix.labels[i])}</th>${row.map(n => `<td>${n}</td>`).join("")}</tr>`).join("") + '</tbody></table></div>';
    } else html += jsonDetails("Confusion matrix",matrix);
    html += flattenTable("Embedded performance",data.embedded) + '<h4>Findings</h4>' + (data.analysis.length ? `<ul>${data.analysis.map(text => `<li>${esc(text)}</li>`).join("")}</ul>` : '<p>No measured findings yet.</p>');
    html += jsonDetails("Model & device",{model:data.model,device:data.device,run:data.run}) + jsonDetails("Other / custom data",data.other);
    el("report").innerHTML = html;
  }
  async function detail() {
    const id = selected; if (!id) return;
    const r = await request(`/runs/${id}`); if (selected !== id) return;
    el("run-name").textContent = r.run_name || r.run_id; el("run-meta").textContent = `${r.device_id} · ${r.mode || "unknown"} · ${r.status}`;
    stats("live-stats",[["Samples",fmt(r.samples,0)],["Labelled accuracy",pct(r.accuracy)],["Confidence",pct(r.mean_confidence)],["Mean inference",`${fmt(r.mean_inference_ms)} ms`],["Duration",`${fmt(r.duration_seconds)} s`]]);
    await results(id);
    const [latest,snapshots,events] = await Promise.all([request(`/runs/${id}/results?page_size=200`),request(`/runs/${id}/snapshots`),request(`/runs/${id}/events`)]);
    if (selected !== id) return;
    chartRows = latest.items.reverse(); chartOptions();
    el("observations").innerHTML = jsonDetails("Latest snapshots (up to 500)",snapshots.items) + jsonDetails("Latest events (up to 500)",events.items);
    if ((!reportIsFinal && !active(r)) || (Date.now()-lastMetrics > 15000 && active(r)) || !lastMetrics) await metrics(id);
  }
  async function refresh() {
    if (!token) return;
    if (refreshing) {refreshAgain=true;return;}
    const session=generation; refreshing=true;
    try {await overview();if (session===generation) await detail();}
    catch (error) {if (session===generation) message(error.message);}
    finally {refreshing=false;if (refreshAgain && session===generation) {refreshAgain=false;schedule();}}
  }
  function schedule() {if (!timer) timer=setTimeout(() => {timer=null;refresh();},1000);}
  function connectSocket(session) {
    if (session!==generation || !token) return;
    const url=new URL(`${OTA_API_BASE}/api/tinyml/ws`);url.protocol=url.protocol==="https:" ? "wss:" : "ws:";
    const ws=new WebSocket(url);socket=ws;
    ws.onopen=() => ws.send(JSON.stringify({token}));
    ws.onmessage=event => {
      if (session!==generation) return;
      try {
        const data=JSON.parse(event.data);
        if (data.type==="tinyml.connected") {reconnectDelay=1000;connectionState("Live","live");message("Live updates connected");}
        if (data.type==="tinyml.report.ready" && data.run_id===selected) reportIsFinal=false;
        schedule();
      } catch {message("Invalid live update received.");}
    };
    ws.onclose=() => {
      if (session!==generation || !token) return;
      connectionState("Reconnecting…","pending");retry=setTimeout(() => connectSocket(session),reconnectDelay);reconnectDelay=Math.min(reconnectDelay*2,30000);
    };
    ws.onerror=() => message("Live connection unavailable. Check the server and allowed origin.");
  }
  function disconnect() {
    generation++;clearTimeout(retry);clearTimeout(timer);timer=null;token="";socket?.close();socket=null;
    selected=null;el("detail").classList.add("hidden");
    connectionState("Disconnected");connectedControls(false);showRunsState(0,false);el("overview").innerHTML="";
    message("Not connected");
  }
  document.getElementById("nav-tinyml").addEventListener("click",event => {event.preventDefault();showSection("tinyml-section","tinyml");});
  el("connect-form").addEventListener("submit",guarded(async () => {
    disconnect();token=el("token").value.trim();connectionState("Connecting","pending");message("Checking access…");
    let devices;
    try {devices=await request("/devices");}
    catch(error){token="";connectionState("Access denied");connectedControls(false);throw error;}
    el("device").innerHTML='<option value="">All devices</option>'+devices.map(d => `<option value="${esc(d.device_id)}">${esc(d.name || d.device_id)}</option>`).join("");
    connectedControls(true);message("Workspace loaded");await refresh();connectSocket(generation);
  }));
  el("disconnect").addEventListener("click",disconnect);
  el("empty-connect").addEventListener("click",guarded(async () => {
    if(token){await overview();return;}
    el("token").focus();
  }));
  el("run-filter").addEventListener("submit",guarded(async () => {runPage=1;await overview();}));
  el("result-filter").addEventListener("submit",guarded(async () => {resultPage=1;if(selected) await results(selected);}));
  el("runs").addEventListener("click",guarded(async event => {
    const button=event.target.closest("button[data-run]");if(!button) return;
    selected=button.dataset.run;resultPage=1;lastMetrics=0;reportIsFinal=false;
    el("detail").classList.remove("hidden");el("report").textContent="Loading…";await detail();
  }));
  for(const [name,direction] of [["prev",-1],["next",1]]) {
    el(`runs-${name}`).addEventListener("click",guarded(async () => {runPage=Math.max(1,runPage+direction);await overview();}));
    el(`results-${name}`).addEventListener("click",guarded(async () => {resultPage=Math.max(1,resultPage+direction);if(selected) await results(selected);}));
  }
  el("signal").addEventListener("change",drawChart);
  el("metrics-refresh").addEventListener("click",guarded(async () => {if(selected) await metrics(selected);}));
  for(const [type,path] of [["json","report/json"],["csv","results.csv"]]) el(`export-${type}`).addEventListener("click",guarded(async () => {
    if(!selected) return;
    const response=await request(`/runs/${selected}/${path}`,true),url=URL.createObjectURL(await response.blob()),a=document.createElement("a");
    a.href=url;a.download=`tinyml-${selected}.${type}`;a.click();setTimeout(() => URL.revokeObjectURL(url),1000);
  }));
  window.addEventListener("pagehide",disconnect);
  connectedControls(false);showRunsState(0,false);
})();
