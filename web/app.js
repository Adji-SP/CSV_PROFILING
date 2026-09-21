/* =====================================================
   CSV Profiler — app.js
   - localStorage + sidecar JSON persistence
   - Sessions table from GET /sessions
   - Cleaning panel from dataset metadata
   - POST /clean -> pandas operations + cleaned CSV download
===================================================== */

const API_BASE    = location.protocol === "https:" ? location.origin : "http://localhost:5000";
const SESSION_KEY = "csv_profiler_current_session";

// -- DOM refs --------------------------------------------------
const dropZone      = document.getElementById("drop-zone");
const fileInput     = document.getElementById("file-input");
const browseTrigger = document.getElementById("browse-trigger");
const fileCard      = document.getElementById("file-card");
const fileNameEl    = document.getElementById("file-name");
const fileSizeEl    = document.getElementById("file-size");
const removeFileBtn = document.getElementById("remove-file");
const analyseBtn    = document.getElementById("analyse-btn");
const newAnalysisBtn= document.getElementById("new-analysis");
const navProfiler   = document.getElementById("nav-profiler");
const navRuns       = document.getElementById("nav-runs");
const navOta        = document.getElementById("nav-ota");
const navDevices    = document.getElementById("nav-devices");
const navBuilds     = document.getElementById("nav-builds");
const breadcrumbCurrent = document.getElementById("breadcrumb-current");

const progressBar   = document.getElementById("progress-bar");
const progressPct   = document.getElementById("progress-pct");
const progressLabel = document.getElementById("progress-label");
const stepList      = document.getElementById("step-list");

const reportFrame    = document.getElementById("report-frame");
const openNewTabBtn  = document.getElementById("open-new-tab");
const reportMetaText = document.getElementById("report-meta-text");
const resetBtn       = document.getElementById("reset-btn");

const cleaningPanelBody = document.getElementById("cleaning-panel-body");
const applyCleanBtn     = document.getElementById("apply-clean-btn");
const cleanResult       = document.getElementById("clean-result");
const cleanResultBody   = document.getElementById("clean-result-body");
const downloadLink      = document.getElementById("download-link");

// Playground refs
const pgOperation  = document.getElementById("pg-operation");
const pgColumn     = document.getElementById("pg-column");
const pgColumn2    = document.getElementById("pg-column-2");
const pgValue      = document.getElementById("pg-value");
const pgIndexes    = document.getElementById("pg-indexes");
const pgLimit      = document.getElementById("pg-limit");
const pgChartType  = document.getElementById("pg-chart-type");
const pgOperator   = document.getElementById("pg-operator");
const pgRun        = document.getElementById("pg-run");
const pgApply      = document.getElementById("pg-apply");
const pgStatus     = document.getElementById("playground-status");
const pgTableWrap  = document.getElementById("pg-table-wrap");
const pgChart      = document.getElementById("pg-chart");

const sessionsEmpty  = document.getElementById("sessions-empty");
const sessionsTable  = document.getElementById("sessions-table");
const sessionsTbody  = document.getElementById("sessions-tbody");
const clearAllBtn    = document.getElementById("clear-all-btn");

const confirmOverlay = document.getElementById("confirm-overlay");
const confirmMsg     = document.getElementById("confirm-msg");
const confirmOk      = document.getElementById("confirm-ok");
const confirmCancel  = document.getElementById("confirm-cancel");

const toast = document.getElementById("toast");

// -- Progress steps --------------------------------------------
// Four honest stages; ydata_profiling does not expose real sub-step callbacks.
const STEPS = [
  { key: "prepare", label: "Preparing dataset" },
  { key: "profile", label: "Generating profiling report" },
  { key: "render",  label: "Rendering report" },
  { key: "done",    label: "Finalising" },
];
const STEP_THRESHOLDS = [15, 75, 95, 100];

function buildStepList() {
  stepList.innerHTML = "";
  STEPS.forEach((s) => {
    const el = document.createElement("div");
    el.className = "step-item";
    el.id = `step-${s.key}`;
    el.innerHTML = `<span class="step-dot"></span><span>${s.label}</span>`;
    stepList.appendChild(el);
  });
}

function updateSteps(pct) {
  STEPS.forEach((s, i) => {
    const el = document.getElementById(`step-${s.key}`);
    if (!el) return;
    if (pct >= STEP_THRESHOLDS[i])                    el.className = "step-item done";
    else if (i === 0 || pct >= STEP_THRESHOLDS[i-1]) el.className = "step-item active";
    else                                               el.className = "step-item";
  });
}

// -- State -----------------------------------------------------
let selectedFile   = null;
let currentSession = null;

// -- Toast -----------------------------------------------------
let toastTimer;
function showToast(msg, type = "", duration = 4000) {
  toast.textContent = msg;
  toast.className   = `toast ${type} show`;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => toast.classList.remove("show"), duration);
}

// -- Format helpers --------------------------------------------
function formatBytes(b) {
  if (b < 1024)    return `${b} B`;
  if (b < 1048576) return `${(b/1024).toFixed(1)} KB`;
  return `${(b/1048576).toFixed(2)} MB`;
}
function fmtDateTime(iso) {
  if (!iso) return "-";
  try {
    const d = new Date(iso);
    return d.toLocaleDateString() + " " +
           d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  } catch { return iso; }
}

// -- Session persistence ---------------------------------------
function saveLocalSession(session) {
  currentSession = session;
  try { localStorage.setItem(SESSION_KEY, JSON.stringify(session)); } catch {}
}
function clearLocalSession() {
  currentSession = null;
  try { localStorage.removeItem(SESSION_KEY); } catch {}
}

// -- Restore session on page load ------------------------------
// Verifies against server, merges cached metadata for old sessions
// (pre-sidecar reports have null metadata on the server side).
async function tryRestoreSession() {
  let cached = null;
  try {
    const raw = localStorage.getItem(SESSION_KEY);
    if (raw) cached = JSON.parse(raw);
  } catch {}

  if (!cached?.job_id) return;

  try {
    const res  = await fetch(`${API_BASE}/session/${cached.job_id}`);
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const data = await res.json();
    if (data.exists) {
      const session = {
        ...data,
        metadata:   data.metadata   || cached.metadata,
        filename:   data.filename   || cached.filename,
        created_at: data.created_at || cached.created_at,
      };
      currentSession = session;
      saveLocalSession(session);
      showReport(session);
    } else {
      clearLocalSession();
    }
  } catch {
    // Server unreachable - restore from localStorage directly
    if (cached.report_url) {
      currentSession = cached;
      showReport(cached);
    }
  }
}

// -- Sessions table --------------------------------------------
async function loadSessionsTable() {
  try {
    const res  = await fetch(`${API_BASE}/sessions`);
    const list = await res.json();

    if (!list.length) {
      sessionsEmpty.classList.remove("hidden");
      sessionsTable.classList.add("hidden");
      return;
    }

    sessionsEmpty.classList.add("hidden");
    sessionsTable.classList.remove("hidden");
    sessionsTbody.innerHTML = "";

    list.forEach((s) => {
      const tr = document.createElement("tr");
      const shortId = s.job_id.slice(0, 8);
      tr.innerHTML = `
        <td>${fmtDateTime(s.created_at)}</td>
        <td title="${esc(s.filename)}">${truncate(s.filename, 22)}</td>
        <td>${s.rows != null ? Number(s.rows).toLocaleString() : "-"}</td>
        <td>${s.cols ?? "-"}</td>
        <td><span class="session-id-chip">${shortId}...</span></td>
        <td>
          <div class="session-actions">
            <button class="btn-sm" data-load="${s.job_id}">Load</button>
            <button class="btn-ghost-sm" data-delete="${s.job_id}">x</button>
          </div>
        </td>`;
      sessionsTbody.appendChild(tr);
    });

    sessionsTbody.querySelectorAll("[data-load]").forEach((btn) => {
      btn.addEventListener("click", () => loadSessionById(btn.dataset.load));
    });
    sessionsTbody.querySelectorAll("[data-delete]").forEach((btn) => {
      btn.addEventListener("click", () => {
        showConfirm("Delete this session?", async () => {
          await fetch(`${API_BASE}/reset/${btn.dataset.delete}`, { method: "DELETE" });
          if (currentSession?.job_id === btn.dataset.delete) {
            clearLocalSession();
            showSection("upload-section");
          }
          loadSessionsTable();
          showToast("Session deleted.", "success");
        });
      });
    });

  } catch {
    // server not reachable
  }
}

async function loadSessionById(jobId) {
  try {
    const res  = await fetch(`${API_BASE}/session/${jobId}`);
    const data = await res.json();
    if (data.exists) {
      currentSession = data;
      saveLocalSession(data);
      showReport(data);
    } else {
      showToast("Report file not found on disk.", "error");
      loadSessionsTable();
    }
  } catch {
    showToast("Could not reach server.", "error");
  }
}

function truncate(str, n) {
  return str.length > n ? str.slice(0, n - 1) + "..." : str;
}

// -- File selection --------------------------------------------
function selectFile(file) {
  if (!file) return;
  if (!file.name.toLowerCase().endsWith(".csv")) {
    showToast("Only .csv files are supported.", "error"); return;
  }
  selectedFile = file;
  fileNameEl.textContent = file.name;
  fileSizeEl.textContent = formatBytes(file.size);
  fileCard.classList.remove("hidden");
  analyseBtn.disabled = false;
}
function clearFile() {
  selectedFile = null;
  fileInput.value = "";
  fileCard.classList.add("hidden");
  analyseBtn.disabled = true;
}

// -- Drag & Drop -----------------------------------------------
dropZone.addEventListener("dragover",  (e) => { e.preventDefault(); dropZone.classList.add("drag-over"); });
["dragleave","dragend"].forEach((ev) => dropZone.addEventListener(ev, () => dropZone.classList.remove("drag-over")));
dropZone.addEventListener("drop", (e) => { e.preventDefault(); dropZone.classList.remove("drag-over"); selectFile(e.dataTransfer.files[0]); });
dropZone.addEventListener("click",   () => fileInput.click());
dropZone.addEventListener("keydown", (e) => { if (e.key==="Enter"||e.key===" ") { e.preventDefault(); fileInput.click(); } });
browseTrigger.addEventListener("click", (e) => { e.stopPropagation(); fileInput.click(); });
fileInput.addEventListener("change", () => selectFile(fileInput.files[0]));
removeFileBtn.addEventListener("click", (e) => { e.stopPropagation(); clearFile(); });

navProfiler?.addEventListener("click", (e) => {
  e.preventDefault();
  showSection("upload-section", "profiler");
  window.scrollTo({ top: 0, behavior: "smooth" });
});

navRuns?.addEventListener("click", (e) => {
  e.preventDefault();
  showSection("upload-section", "runs");
  document.querySelector(".sessions-card")?.scrollIntoView({ behavior: "smooth", block: "start" });
});

navOta?.addEventListener("click", (e) => {
  e.preventDefault();
  showSection("ota-section", "ota");
  if (typeof loadOtaDashboard === "function") loadOtaDashboard();
});

navDevices?.addEventListener("click", (e) => {
  e.preventDefault();
  showSection("devices-section", "devices");
  if (typeof loadDevices === "function") loadDevices();
});

navBuilds?.addEventListener("click", (e) => {
  e.preventDefault();
  showSection("builds-section", "builds");
  if (typeof loadBuildHistory === "function") loadBuildHistory();
});

// -- Section helpers -------------------------------------------
function showSection(id, activeNav = null) {
  ["upload-section","progress-section","report-section","ota-section","devices-section","builds-section","console-section"].forEach((sid) => {
    document.getElementById(sid).classList.toggle("hidden", sid !== id);
  });
  newAnalysisBtn.style.display = id === "report-section" ? "inline-block" : "none";
  const sectionMeta = {
    "upload-section":   { nav: activeNav || "profiler", title: activeNav === "runs" ? "CSV Runs" : "CSV Profiler" },
    "progress-section": { nav: "profiler", title: "Profiling Progress" },
    "report-section":   { nav: "profiler", title: "EDA Report" },
    "ota-section":      { nav: "ota", title: "OTA Firmware" },
    "devices-section":  { nav: "devices", title: "Devices" },
    "builds-section":   { nav: "builds", title: "Build History" },
    "console-section":  { nav: "console", title: "Remote Console" },
  }[id];
  setActiveNav(sectionMeta?.nav || "profiler");
  if (breadcrumbCurrent && sectionMeta) breadcrumbCurrent.textContent = sectionMeta.title;
  if (id === "upload-section") loadSessionsTable();
}

function setActiveNav(active) {
  document.getElementById("nav-console")?.classList.toggle("active", active === "console");
  navProfiler?.classList.toggle("active", active === "profiler");
  navRuns?.classList.toggle("active", active === "runs");
  navOta?.classList.toggle("active", active === "ota");
  navDevices?.classList.toggle("active", active === "devices");
  navBuilds?.classList.toggle("active", active === "builds");
}

function setProgress(pct, label) {
  const v = Math.max(0, Math.min(100, pct));
  progressBar.style.width = `${v}%`;
  progressBar.parentElement.setAttribute("aria-valuenow", v);
  progressPct.textContent = `${v}%`;
  if (label) progressLabel.textContent = label;
  updateSteps(v);
}

// -- Show report -----------------------------------------------
function reportUrlFor(session, cacheBust = false) {
  const url = API_BASE + session.report_url;
  return cacheBust ? `${url}?v=${Date.now()}` : url;
}

function updateReportMeta(session) {
  const parts = [session.filename || "Unknown file"];
  const meta  = session.metadata || {};
  if (meta.rows) parts.push(`${Number(meta.rows).toLocaleString()} rows`);
  if (meta.cols) parts.push(`${meta.cols} columns`);
  if (session.created_at) parts.push(`Generated ${fmtDateTime(session.created_at)}`);
  if (session.cleaned_at) parts.push(`Cleaned ${fmtDateTime(session.cleaned_at)}`);
  reportMetaText.textContent = parts.join(" | ");
}

function showReport(session) {
  const reportUrl = reportUrlFor(session);
  reportFrame.src = reportUrl;
  openNewTabBtn.onclick = () => window.open(reportUrl, "_blank");

  const meta = session.metadata || {};
  updateReportMeta(session);

  if (meta && Object.keys(meta).length) {
    buildCleaningPanel(meta, session.job_id);
    applyCleanBtn.disabled = false;
  }

  cleanResult.classList.add("hidden");
  downloadLink.style.display = "none";

  showSection("report-section");

  // Auto-run a preview so the Playground table is populated on load
  setTimeout(() => runPlayground("preview", false), 100);
}

// -- Analyse (upload + SSE) ------------------------------------
analyseBtn.addEventListener("click", async () => {
  if (!selectedFile) return;

  buildStepList();
  setProgress(0, "Uploading file...");
  showSection("progress-section");

  const form = new FormData();
  form.append("file", selectedFile);

  let jobId;
  try {
    const res  = await fetch(`${API_BASE}/upload`, { method: "POST", body: form });
    const data = await res.json();
    if (!res.ok || data.error) throw new Error(data.error || "Upload failed");
    jobId = data.job_id;
  } catch (err) {
    showToast(`Upload error: ${err.message}`, "error", 6000);
    showSection("upload-section");
    return;
  }

  setProgress(5, "File uploaded. Starting profiler...");

  const evtSource = new EventSource(`${API_BASE}/progress/${jobId}`);

  evtSource.onmessage = (e) => {
    let msg;
    try { msg = JSON.parse(e.data); } catch { return; }
    setProgress(msg.progress, msg.status);

    if (msg.error) {
      evtSource.close();
      showToast(`Error: ${msg.error}`, "error", 8000);
      showSection("upload-section");
      return;
    }

    if (msg.progress >= 100 && msg.report_url) {
      evtSource.close();

      // Fallback built from SSE data; used if the sidecar fetch fails.
      const fallbackSession = {
        job_id:     jobId,
        report_url: msg.report_url,
        filename:   selectedFile?.name || "unknown.csv",
        created_at: new Date().toISOString(),
        metadata:   msg.metadata || null,
        exists:     true,
      };

      fetch(`${API_BASE}/session/${jobId}`)
        .then((r) => {
          if (!r.ok) throw new Error(`HTTP ${r.status}`);
          return r.json();
        })
        .then((data) => {
          let session = fallbackSession;
          if (data && data.exists) {
            session = {
              ...data,
              metadata:   data.metadata   || fallbackSession.metadata,
              filename:   data.filename   || fallbackSession.filename,
              created_at: data.created_at || fallbackSession.created_at,
            };
          }
          saveLocalSession(session);
          setTimeout(() => {
            showReport(session);
            showToast("Report generated.", "success");
            loadSessionsTable();
          }, 500);
        })
        .catch(() => {
          saveLocalSession(fallbackSession);
          setTimeout(() => {
            showReport(fallbackSession);
            showToast("Report generated.", "success");
            loadSessionsTable();
          }, 500);
        });
    }
  };

  evtSource.onerror = () => {
    evtSource.close();
    if (!document.getElementById("progress-section").classList.contains("hidden")) {
      showToast("Connection lost.", "error", 6000);
      showSection("upload-section");
    }
  };
});

// -- New Run button --------------------------------------------
newAnalysisBtn.addEventListener("click", () => {
  clearFile();
  reportFrame.src = "about:blank";
  cleanResult.classList.add("hidden");
  setProgress(0, "Initialising...");
  showSection("upload-section");
});

// -- Reset button ----------------------------------------------
resetBtn.addEventListener("click", () => {
  showConfirm("Delete this report and clear the session?", async () => {
    if (currentSession?.job_id) {
      try { await fetch(`${API_BASE}/reset/${currentSession.job_id}`, { method: "DELETE" }); } catch {}
    }
    clearLocalSession();
    clearFile();
    reportFrame.src = "about:blank";
    cleanResult.classList.add("hidden");
    setProgress(0, "Initialising...");
    showSection("upload-section");
    showToast("Session cleared.", "success");
  });
});

// -- Clear All sessions ----------------------------------------
clearAllBtn.addEventListener("click", () => {
  showConfirm("Delete ALL sessions and reports from disk?", async () => {
    await fetch(`${API_BASE}/reset-all`, { method: "DELETE" });
    clearLocalSession();
    loadSessionsTable();
    showToast("All sessions cleared.", "success");
  });
});

// =====================================================
// Cleaning Panel Builder
// =====================================================
function buildCleaningPanel(meta, jobId) {
  cleaningPanelBody.innerHTML = "";
  let issueSections = 0;

  if (Object.keys(meta.missing_cols || {}).length) {
    cleaningPanelBody.appendChild(buildMissingSection(meta));
    issueSections++;
  }
  if (meta.duplicate_rows > 0) {
    cleaningPanelBody.appendChild(buildDuplicatesSection(meta));
    issueSections++;
  }
  if ((meta.constant_cols || []).length) {
    cleaningPanelBody.appendChild(buildConstantColsSection(meta));
    issueSections++;
  }

  // Detected type conversions: numeric-like strings + datetime-like strings (one section, no duplication)
  const dtCols = (meta.datetime_cols || []).filter(c => !(meta.numeric_cols || []).includes(c));
  const typeCandidates = [
    ...(meta.numeric_like_cols || []).map(c => ({ col: c, fix: "numeric" })),
    ...dtCols.map(c => ({ col: c, fix: "datetime" })),
  ];
  if (typeCandidates.length) {
    cleaningPanelBody.appendChild(buildTypeSection(typeCandidates));
    issueSections++;
  }

  if ((meta.outlier_cols || []).length) {
    cleaningPanelBody.appendChild(buildOutliersSection(meta));
    issueSections++;
  }

  if (!issueSections && !cleaningPanelBody.children.length) {
    const p = document.createElement("p");
    p.className = "cleaning-empty";
    p.style.padding = "16px";
    p.textContent = "No data quality issues detected in this dataset.";
    cleaningPanelBody.appendChild(p);
  }

  // Numerical formatting is optional, always shown when numeric cols exist
  if ((meta.numeric_cols || []).length) {
    cleaningPanelBody.appendChild(buildNumericsSection(meta));
  }
}

function makeSection(title, badgeText, badgeClass, bodyHtml, openByDefault = false) {
  const div = document.createElement("div");
  div.className = "clean-section" + (openByDefault ? " open" : "");
  div.innerHTML = `
    <div class="clean-section-header">
      <span class="clean-section-title">
        <span>${title}</span>
        <span class="clean-section-badge ${badgeClass}">${badgeText}</span>
      </span>
      <span class="clean-section-chevron">></span>
    </div>
    <div class="clean-section-body">${bodyHtml}</div>`;
  div.querySelector(".clean-section-header").addEventListener("click", () => {
    div.classList.toggle("open");
  });
  return div;
}

function buildMissingSection(meta) {
  const mc   = meta.missing_cols;
  const rows = meta.rows || 1;
  let html = "";
  Object.entries(mc).forEach(([col, count]) => {
    const pct    = ((count / rows) * 100).toFixed(1);
    const isNum  = (meta.numeric_cols || []).includes(col);
    html += `
      <div class="clean-item">
        <input type="checkbox" class="clean-checkbox" checked
               data-missing-col="${esc(col)}" id="miss-chk-${esc(col)}">
        <label class="clean-item-label" for="miss-chk-${esc(col)}" title="${esc(col)}">${esc(col)}</label>
        <span class="clean-item-count">${count} (${pct}%)</span>
        <select class="clean-select" data-missing-strategy="${esc(col)}">
          ${isNum ? `
            <option value="fill_median" selected>Fill median</option>
            <option value="fill_mean">Fill mean</option>
            <option value="fill_zero">Fill 0</option>
            <option value="drop_rows">Drop rows</option>
          ` : `
            <option value="fill_mode" selected>Fill mode</option>
            <option value="fill_unknown">Fill "Unknown"</option>
            <option value="drop_rows">Drop rows</option>
          `}
        </select>
      </div>`;
  });
  const count = Object.keys(mc).length;
  return makeSection("Missing Values", `${count} col${count > 1 ? "s" : ""}`, "", html, true);
}

function buildDuplicatesSection(meta) {
  const n    = meta.duplicate_rows;
  const html = `
    <p class="clean-info-text">${n.toLocaleString()} duplicate row${n > 1 ? "s" : ""} found.</p>
    <label class="clean-radio-label" style="gap:8px;">
      <input type="checkbox" class="clean-checkbox" id="dup-chk" checked>
      Remove all duplicates
    </label>`;
  return makeSection("Duplicate Rows", `${n}`, "warn", html, true);
}

function buildConstantColsSection(meta) {
  const cols = meta.constant_cols;
  let html = `<p class="clean-info-text">These columns have zero variance and carry no information.</p>`;
  cols.forEach((col) => {
    html += `
      <div class="clean-item">
        <input type="checkbox" class="clean-checkbox" checked
               data-drop-col="${esc(col)}" id="drop-chk-${esc(col)}">
        <label class="clean-item-label" for="drop-chk-${esc(col)}" title="${esc(col)}">${esc(col)}</label>
        <span class="clean-item-count">constant</span>
      </div>`;
  });
  return makeSection("Constant Columns", `${cols.length}`, "", html, false);
}

function buildTypeSection(candidates) {
  let html = `<p class="clean-info-text">Columns that need type conversion.</p>`;
  candidates.forEach(({ col, fix }) => {
    html += `
      <div class="clean-item">
        <input type="checkbox" class="clean-checkbox" checked
               data-fix-col="${esc(col)}" data-fix-type="${fix}"
               id="fix-chk-${esc(col)}">
        <label class="clean-item-label" for="fix-chk-${esc(col)}" title="${esc(col)}">${esc(col)}</label>
        <span class="clean-item-count">-> ${fix}</span>
      </div>`;
  });
  return makeSection("Detected Type Conversions", `${candidates.length}`, "warn", html, false);
}

function buildNumericsSection(meta) {
  const cols = meta.numeric_cols;
  const html = `
    <p class="clean-info-text">Optional scaling for ${cols.length} numeric column${cols.length > 1 ? "s" : ""}.</p>
    <div class="clean-radio-group">
      <label class="clean-radio-label"><input type="radio" class="clean-radio" name="num-fmt" value="none" checked> None</label>
      <label class="clean-radio-label"><input type="radio" class="clean-radio" name="num-fmt" value="normalize"> Normalize [0,1]</label>
      <label class="clean-radio-label"><input type="radio" class="clean-radio" name="num-fmt" value="standardize"> Standardize (z)</label>
      <label class="clean-radio-label"><input type="radio" class="clean-radio" name="num-fmt" value="round_2"> Round 2 dp</label>
    </div>`;
  return makeSection("Numerical Formatting", `${cols.length} cols`, "ok", html, false);
}

function buildOutliersSection(meta) {
  const cols = meta.outlier_cols;
  let html = `
    <p class="clean-info-text">Outliers detected via IQR in ${cols.length} column${cols.length > 1 ? "s" : ""}.</p>
    <div class="clean-radio-group" style="margin-bottom:8px;">
      <label class="clean-radio-label"><input type="radio" class="clean-radio" name="outlier-method" value="cap_iqr" checked> Cap to IQR bounds</label>
      <label class="clean-radio-label"><input type="radio" class="clean-radio" name="outlier-method" value="remove_rows"> Remove rows</label>
    </div>`;
  cols.forEach((col) => {
    html += `
      <div class="clean-item">
        <input type="checkbox" class="clean-checkbox" checked
               data-outlier-col="${esc(col)}" id="out-chk-${esc(col)}">
        <label class="clean-item-label" for="out-chk-${esc(col)}" title="${esc(col)}">${esc(col)}</label>
      </div>`;
  });
  return makeSection("Outlier Columns", `${cols.length}`, "warn", html, false);
}

// -- Collect cleaning ops from UI ------------------------------
function collectOps() {
  const ops  = {};
  const meta = currentSession?.metadata || {};

  // Missing values
  const missingOps = {};
  cleaningPanelBody.querySelectorAll("[data-missing-col]").forEach((chk) => {
    if (chk.checked) {
      const col = chk.dataset.missingCol;
      const sel = cleaningPanelBody.querySelector(`[data-missing-strategy="${col}"]`);
      missingOps[col] = sel ? sel.value : "fill_median";
    }
  });
  if (Object.keys(missingOps).length) ops.missing = missingOps;

  // Duplicates
  const dupChk = cleaningPanelBody.querySelector("#dup-chk");
  if (dupChk && dupChk.checked) ops.drop_duplicates = true;

  // Constant columns
  const dropCols = [];
  cleaningPanelBody.querySelectorAll("[data-drop-col]").forEach((chk) => {
    if (chk.checked) dropCols.push(chk.dataset.dropCol);
  });
  if (dropCols.length) ops.drop_cols = dropCols;

  // Type conversions (numeric-like strings AND datetime-like strings)
  const fixTypes = {};
  cleaningPanelBody.querySelectorAll("[data-fix-col]").forEach((chk) => {
    if (chk.checked) fixTypes[chk.dataset.fixCol] = chk.dataset.fixType;
  });
  if (Object.keys(fixTypes).length) ops.fix_types = fixTypes;

  // Numerical formatting
  const numFmtRadio = cleaningPanelBody.querySelector("input[name='num-fmt']:checked");
  if (numFmtRadio && numFmtRadio.value !== "none") {
    ops.numeric_format = { cols: meta.numeric_cols || [], method: numFmtRadio.value };
  }

  // Outliers
  const outlierCols = [];
  cleaningPanelBody.querySelectorAll("[data-outlier-col]").forEach((chk) => {
    if (chk.checked) outlierCols.push(chk.dataset.outlierCol);
  });
  const outlierMethod = cleaningPanelBody.querySelector("input[name='outlier-method']:checked");
  if (outlierCols.length) {
    ops.outliers = { cols: outlierCols, method: outlierMethod?.value || "cap_iqr" };
  }

  return ops;
}

// -- Apply cleaning --------------------------------------------
applyCleanBtn.addEventListener("click", async () => {
  const jobId = currentSession?.job_id;
  if (!jobId) return;

  const ops = collectOps();
  if (!Object.keys(ops).length) {
    showToast("Nothing selected to clean.", "error");
    return;
  }

  applyCleanBtn.disabled = true;
  applyCleanBtn.textContent = "Applying...";

  try {
    const res  = await fetch(`${API_BASE}/clean/${jobId}`, {
      method:  "POST",
      headers: { "Content-Type": "application/json" },
      body:    JSON.stringify(ops),
    });
    const data = await res.json();
    if (!res.ok || data.error) throw new Error(data.error || "Cleaning failed");

    const before = data.original_shape;
    const after  = data.cleaned_shape;
    let html = `<div class="clean-result-title">Changes Applied</div>`;
    html += `<div class="clean-shape">${before[0]} rows x ${before[1]} cols -> ${after[0]} rows x ${after[1]} cols</div>`;
    data.changes.forEach((c) => {
      html += `<div class="clean-change-item">${esc(c)}</div>`;
    });
    if (!data.changes.length) {
      html += `<div class="clean-change-item" style="color:#9ca3af;">No rows/columns were modified.</div>`;
    }
    cleanResultBody.innerHTML = html;
    cleanResult.classList.remove("hidden");

    downloadLink.href = API_BASE + data.download_url;
    downloadLink.style.display = "block";

    // Update session metadata and rebuild cleaning panel with fresh metadata
    if (data.metadata) {
      currentSession = {
        ...currentSession,
        metadata:   data.metadata,
        cleaned_at: data.cleaned_at || new Date().toISOString(),
      };
      saveLocalSession(currentSession);
      updateReportMeta(currentSession);
      buildCleaningPanel(data.metadata, jobId);
    }

    showToast(`Cleaning done - ${data.changes.length} change${data.changes.length !== 1 ? "s" : ""}`, "success");
  } catch (err) {
    showToast(`Error: ${err.message}`, "error", 6000);
  } finally {
    applyCleanBtn.disabled = false;
    applyCleanBtn.textContent = "Apply & Download";
  }
});

// -- Confirm modal ---------------------------------------------
let _confirmCb = null;
function showConfirm(msg, onOk) {
  confirmMsg.textContent = msg;
  _confirmCb = onOk;
  confirmOverlay.classList.remove("hidden");
}
confirmOk.addEventListener("click", () => {
  confirmOverlay.classList.add("hidden");
  if (_confirmCb) { _confirmCb(); _confirmCb = null; }
});
confirmCancel.addEventListener("click", () => {
  confirmOverlay.classList.add("hidden");
  _confirmCb = null;
});

// -- HTML escape -----------------------------------------------
function esc(str) {
  return String(str)
    .replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;").replace(/'/g, "&#39;");
}

// =====================================================
// Pandas Playground
// =====================================================
function playgroundPayload(operationOverride, mutate = false) {
  return {
    operation:  operationOverride || pgOperation.value,
    column:     pgColumn.value.trim(),
    column_2:   pgColumn2.value.trim(),
    value:      pgValue.value,
    indexes:    pgIndexes.value,
    limit:      Number(pgLimit.value || 20),
    chart_type: pgChartType.value,
    operator:   pgOperator.value,
    mutate,
  };
}

function renderPlaygroundTable(table) {
  if (!table || !table.columns || !table.rows) {
    pgTableWrap.innerHTML = '<p class="sessions-empty">No table output.</p>';
    return;
  }
  const head = table.columns.map((c) => `<th>${esc(String(c))}</th>`).join("");
  const body = table.rows.map((row) => {
    const cells = table.columns.map((c) => `<td>${esc(String(row[c] ?? ""))}</td>`).join("");
    return `<tr>${cells}</tr>`;
  }).join("");
  pgTableWrap.innerHTML = `<table class="pg-table"><thead><tr>${head}</tr></thead><tbody>${body}</tbody></table>`;
}

function clearPlaygroundChart() {
  if (!pgChart) return;
  const ctx = pgChart.getContext("2d");
  ctx.clearRect(0, 0, pgChart.width, pgChart.height);
  // Hide the chart panel so the table takes full width
  pgChart.closest(".playground-output")?.classList.remove("chart-visible");
}

function drawPlaygroundChart(chart) {
  clearPlaygroundChart();
  if (!chart || !pgChart) return;

  const labels = chart.labels || [];
  const values = (chart.data || []).map(Number);
  if (!labels.length || !values.length) return;

  // Show chart panel (splits layout to 42 | 58)
  pgChart.closest(".playground-output")?.classList.add("chart-visible");

  const ctx    = pgChart.getContext("2d");
  const width  = pgChart.width;
  const height = pgChart.height;
  const pad    = 40;
  const minVal = Math.min(0, ...values);
  const maxVal = Math.max(...values);
  const span   = maxVal - minVal || 1;
  const xFor   = (i) => pad + (i * (width - pad * 2)) / Math.max(1, labels.length - 1);
  const yFor   = (v) => height - pad - ((v - minVal) * (height - pad * 2)) / span;

  // Axes
  ctx.strokeStyle = "#d1d5db";
  ctx.lineWidth = 1;
  ctx.beginPath();
  ctx.moveTo(pad, pad);
  ctx.lineTo(pad, height - pad);
  ctx.lineTo(width - pad, height - pad);
  ctx.stroke();

  // Y-axis labels
  ctx.fillStyle = "#6b7280";
  ctx.font = "11px Inter, sans-serif";
  ctx.textAlign = "right";
  ctx.fillText(maxVal.toFixed ? maxVal.toFixed(2) : String(maxVal), pad - 4, pad + 4);
  ctx.fillText(minVal.toFixed ? minVal.toFixed(2) : String(minVal), pad - 4, height - pad);
  ctx.textAlign = "left";

  if (chart.type === "scatter") {
    ctx.fillStyle = "#6366f1";
    values.forEach((v, i) => {
      ctx.beginPath();
      ctx.arc(xFor(i), yFor(v), 3, 0, Math.PI * 2);
      ctx.fill();
    });
  } else if (chart.type === "line") {
    ctx.strokeStyle = "#6366f1";
    ctx.lineWidth = 2;
    ctx.beginPath();
    values.forEach((v, i) => {
      const x = xFor(i), y = yFor(v);
      if (i === 0) ctx.moveTo(x, y); else ctx.lineTo(x, y);
    });
    ctx.stroke();
  } else {
    // bar
    const barGap = 3;
    const barW   = Math.max(3, (width - pad * 2) / values.length - barGap);
    ctx.fillStyle = "#6366f1";
    values.forEach((v, i) => {
      const x = pad + i * (barW + barGap);
      const y = yFor(v);
      ctx.fillRect(x, y, barW, height - pad - y);
    });
  }

  // X-axis labels (up to 8)
  ctx.fillStyle = "#6b7280";
  ctx.font = "10px Inter, sans-serif";
  const step   = Math.max(1, Math.floor(labels.length / 8));
  const shown  = labels.filter((_, i) => i % step === 0);
  shown.forEach((label, si) => {
    const i = si * step;
    const x = chart.type === "scatter" || chart.type === "line"
      ? xFor(i)
      : pad + i * ((width - pad * 2) / values.length) + (width - pad * 2) / values.length / 2;
    ctx.save();
    ctx.translate(x, height - pad + 10);
    ctx.rotate(-0.4);
    ctx.fillText(String(label).slice(0, 16), 0, 0);
    ctx.restore();
  });
}

async function runPlayground(operationOverride = null, mutate = false) {
  if (!currentSession?.job_id) {
    pgStatus.textContent = "No active session. Load or generate a report first.";
    return;
  }
  const op = operationOverride || pgOperation.value;
  pgStatus.textContent = mutate ? "Applying mutation..." : "Running...";
  if (pgRun)   pgRun.disabled   = true;
  if (pgApply) pgApply.disabled = true;

  try {
    const res  = await fetch(`${API_BASE}/playground/${currentSession.job_id}`, {
      method:  "POST",
      headers: { "Content-Type": "application/json" },
      body:    JSON.stringify(playgroundPayload(op, mutate)),
    });
    const data = await res.json();
    if (!res.ok || data.error) throw new Error(data.error || "Operation failed");

    renderPlaygroundTable(data.table);
    drawPlaygroundChart(data.chart);

    const shape = data.table?.shape;
    pgStatus.textContent = shape
      ? `${shape[0].toLocaleString()} rows x ${shape[1]} cols`
      : "Done";

    if (mutate && data.report_url) {
      currentSession = {
        ...currentSession,
        report_url: data.report_url,
        metadata:   data.metadata   || currentSession.metadata,
        cleaned_at: data.cleaned_at || new Date().toISOString(),
      };
      saveLocalSession(currentSession);
      reportFrame.src = reportUrlFor(currentSession, true);
      openNewTabBtn.onclick = () => window.open(reportUrlFor(currentSession), "_blank");
      updateReportMeta(currentSession);
      if (currentSession.metadata) buildCleaningPanel(currentSession.metadata, currentSession.job_id);
      downloadLink.href = API_BASE + (data.download_url || `/download-cleaned/${currentSession.job_id}`);
      downloadLink.style.display = "block";
      loadSessionsTable();
    }
  } catch (err) {
    pgStatus.textContent = `Error: ${err.message}`;
    showToast(`Playground error: ${err.message}`, "error", 6000);
  } finally {
    if (pgRun)   pgRun.disabled   = false;
    if (pgApply) pgApply.disabled = false;
  }
}

pgRun?.addEventListener("click", () => runPlayground(null, false));

pgApply?.addEventListener("click", () => {
  const mutatingOps = new Set(["drop_columns","drop_rows","set_cell","fillna","rename_column","astype"]);
  if (!mutatingOps.has(pgOperation.value)) {
    showToast("Choose a mutation operation before applying.", "error");
    return;
  }
  showConfirm("Apply this mutation and regenerate the EDA report?", () => runPlayground(null, true));
});

// -- Init ------------------------------------------------------
loadSessionsTable();
tryRestoreSession();
