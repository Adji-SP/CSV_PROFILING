/* =====================================================
   OTA Firmware client - isolated from the Flask CSV API
===================================================== */

const OTA_API_BASE = location.protocol === "https:" ? location.origin : "http://localhost:7000";

const otaProjectInput = document.getElementById("ota-project-input");
const otaTarget = document.getElementById("ota-target");
const otaVersion = document.getElementById("ota-version");
const otaBuildBtn = document.getElementById("ota-build-btn");
const otaBuildState = document.getElementById("ota-build-state");
const otaBuildLog = document.getElementById("ota-build-log");
const otaBuildId = document.getElementById("ota-build-id");
const firmwareTbody = document.getElementById("firmware-tbody");
const firmwareCount = document.getElementById("firmware-count");
const buildsTbody = document.getElementById("builds-tbody");
const deviceGrid = document.getElementById("device-grid");
const devicesEmpty = document.getElementById("devices-empty");
const serviceIndicator = document.getElementById("ota-service-indicator");

let otaFirmware = [];
let otaDevices = [];
let otaBuilds = [];
let activeBuildPoll = null;

async function otaRequest(path, options = {}) {
  let response;
  try {
    response = await fetch(`${OTA_API_BASE}${path}`, options);
  } catch (error) {
    throw new Error(`OTA server is unavailable at ${OTA_API_BASE}`);
  }
  if (response.status === 204) return null;

  const contentType = response.headers.get("content-type") || "";
  const data = contentType.includes("application/json")
    ? await response.json()
    : await response.text();
  if (!response.ok) {
    const error = data?.error;
    const parts = [error?.message || data || `HTTP ${response.status}`];
    if (error?.details) parts.push(error.details);
    if (error?.hint) parts.push(error.hint);
    throw new Error(parts.join(" — "));
  }
  return data;
}

async function getDevices() {
  return otaRequest("/api/ota/devices");
}

async function uploadFirmwareProject(file, target, version) {
  const form = new FormData();
  form.append("project", file);
  form.append("target", target);
  form.append("version", version);
  return otaRequest("/api/ota/projects", { method: "POST", body: form });
}

async function buildFirmware(projectId) {
  return otaRequest(`/api/ota/projects/${encodeURIComponent(projectId)}/build`, {
    method: "POST",
  });
}

async function getBuildStatus(buildId) {
  return otaRequest(`/api/ota/builds/${encodeURIComponent(buildId)}`);
}

async function getFirmwareList() {
  return otaRequest("/api/ota/firmware");
}

async function deployFirmware(deviceId, firmwareId) {
  return otaRequest(`/api/ota/devices/${encodeURIComponent(deviceId)}/deploy`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ firmware_id: firmwareId }),
  });
}

async function checkOtaService() {
  try {
    const status = await otaRequest("/api/ota/status");
    serviceIndicator.className = "service-indicator online";
    serviceIndicator.innerHTML = '<span class="service-dot"></span> OTA online';
    serviceIndicator.title = `${status.service} ${status.version}\nFirmware URL: ${status.public_base_url}`;
    return status;
  } catch (error) {
    serviceIndicator.className = "service-indicator offline";
    serviceIndicator.innerHTML = '<span class="service-dot"></span> OTA offline';
    serviceIndicator.title = error.message;
    return null;
  }
}

function setOtaBuildState(status, label = null) {
  const normalized = String(status || "idle").toLowerCase();
  otaBuildState.className = `status-badge status-${normalized}`;
  otaBuildState.textContent = label || normalized.charAt(0).toUpperCase() + normalized.slice(1);
}

function otaDate(iso) {
  if (!iso) return "—";
  const value = new Date(iso);
  return Number.isNaN(value.getTime()) ? iso : value.toLocaleString();
}

function otaBytes(value) {
  const bytes = Number(value || 0);
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 ** 2) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 ** 2).toFixed(2)} MB`;
}

function shortHash(value) {
  return value ? `${value.slice(0, 10)}…` : "—";
}

function otaTargetLabel(value) {
  return String(value).toLowerCase() === "esp32s3" ? "ESP32-S3" : String(value).toUpperCase();
}

function badge(status) {
  const normalized = String(status || "unknown").toLowerCase();
  return `<span class="status-badge status-${esc(normalized)}">${esc(normalized)}</span>`;
}

async function loadOtaDashboard() {
  await checkOtaService();
  await Promise.all([loadFirmware(), loadBuildHistory(false), loadDevices(false)]);
}

async function loadFirmware(showError = true) {
  try {
    otaFirmware = await getFirmwareList();
    renderFirmware();
  } catch (error) {
    firmwareTbody.innerHTML = `<tr><td colspan="9" class="table-empty error-text">${esc(error.message)}</td></tr>`;
    if (showError) showToast(error.message, "error", 6000);
  }
}

function renderFirmware() {
  firmwareCount.textContent = `${otaFirmware.length} image${otaFirmware.length === 1 ? "" : "s"}`;
  if (!otaFirmware.length) {
    firmwareTbody.innerHTML = '<tr><td colspan="9" class="table-empty">No firmware images have been built yet.</td></tr>';
    return;
  }
  firmwareTbody.innerHTML = otaFirmware.map((firmware) => `
    <tr>
      <td><strong>v${esc(firmware.version)}</strong></td>
      <td>${esc(otaTargetLabel(firmware.target))}</td>
      <td><span class="session-id-chip" title="${esc(firmware.build_id)}">${esc(firmware.build_id.slice(0, 8))}</span></td>
      <td class="filename-cell" title="${esc(firmware.filename)}">${esc(firmware.filename)}</td>
      <td>${otaBytes(firmware.size)}</td>
      <td><code title="${esc(firmware.sha256)}">${shortHash(esc(firmware.sha256))}</code></td>
      <td>${otaDate(firmware.created_at)}</td>
      <td>${badge(firmware.status)}</td>
      <td><div class="row-actions">
        <a class="btn-sm action-link" href="${esc(firmware.download_url)}">Download</a>
        <button class="btn-primary-sm" data-firmware-deploy="${esc(firmware.firmware_id)}">Deploy</button>
        <button class="btn-sm" data-firmware-detail="${esc(firmware.firmware_id)}">Details</button>
        <button class="btn-ghost-sm" data-firmware-delete="${esc(firmware.firmware_id)}">Delete</button>
      </div></td>
    </tr>`).join("");

  firmwareTbody.querySelectorAll("[data-firmware-detail]").forEach((button) => {
    button.addEventListener("click", () => showFirmwareDetails(button.dataset.firmwareDetail));
  });
  firmwareTbody.querySelectorAll("[data-firmware-deploy]").forEach((button) => {
    button.addEventListener("click", () => openDeployDialog(null, button.dataset.firmwareDeploy));
  });
  firmwareTbody.querySelectorAll("[data-firmware-delete]").forEach((button) => {
    button.addEventListener("click", () => confirmFirmwareDelete(button.dataset.firmwareDelete));
  });
}

function showFirmwareDetails(firmwareId) {
  const firmware = otaFirmware.find((item) => item.firmware_id === firmwareId);
  if (!firmware) return;
  const rows = [
    ["Firmware ID", firmware.firmware_id],
    ["Project", firmware.project],
    ["Version", firmware.version],
    ["Target", firmware.target],
    ["Build ID", firmware.build_id],
    ["Filename", firmware.filename],
    ["Size", `${otaBytes(firmware.size)} (${Number(firmware.size).toLocaleString()} bytes)`],
    ["SHA-256", firmware.sha256],
    ["Created", otaDate(firmware.created_at)],
    ["Download URL", firmware.download_url],
  ];
  document.getElementById("ota-detail-body").innerHTML = rows
    .map(([key, value]) => `<div><dt>${esc(key)}</dt><dd>${esc(value)}</dd></div>`)
    .join("");
  document.getElementById("ota-detail-overlay").classList.remove("hidden");
}

function confirmFirmwareDelete(firmwareId) {
  showConfirm("Delete this firmware binary? Deployed or desired firmware cannot be deleted.", async () => {
    try {
      await otaRequest(`/api/ota/firmware/${encodeURIComponent(firmwareId)}`, { method: "DELETE" });
      showToast("Firmware deleted.", "success");
      await loadFirmware();
    } catch (error) {
      showToast(error.message, "error", 7000);
    }
  });
}

async function loadDevices(showError = true) {
  try {
    otaDevices = await getDevices();
    renderDevices();
  } catch (error) {
    if (showError) showToast(error.message, "error", 6000);
    deviceGrid.innerHTML = `<div class="card empty-state error-text">${esc(error.message)}</div>`;
  }
}

function renderDevices() {
  devicesEmpty.classList.toggle("hidden", otaDevices.length !== 0);
  if (!otaDevices.length) {
    deviceGrid.innerHTML = "";
    return;
  }
  deviceGrid.innerHTML = otaDevices.map((device) => {
    const progress = Math.max(0, Math.min(100, Number(device.ota_progress || 0)));
    const seen = new Date(device.last_seen);
    const stale = !Number.isNaN(seen.getTime()) && Date.now() - seen.getTime() > 90_000;
    const displayStatus = stale && device.status === "online" ? "offline" : device.status;
    return `<article class="card device-card">
      <div class="device-card-header">
        <div><h2>${esc(device.name)}</h2><code>${esc(device.device_id)}</code></div>
        ${badge(displayStatus)}
      </div>
      <dl class="device-meta">
        <div><dt>IP address</dt><dd>${esc(device.ip_address)}</dd></div>
        <div><dt>Chip</dt><dd>${esc(otaTargetLabel(device.chip))}</dd></div>
        <div><dt>Current</dt><dd>v${esc(device.current_version)}</dd></div>
        <div><dt>Target</dt><dd>${device.desired_version ? `v${esc(device.desired_version)}` : "—"}</dd></div>
        <div><dt>Last seen</dt><dd>${otaDate(device.last_seen)}</dd></div>
      </dl>
      <div class="ota-progress-block">
        <div class="prog-meta"><span class="prog-status">OTA: ${esc(device.ota_status)}</span><span class="prog-pct">${progress}%</span></div>
        <div class="prog-track"><div class="prog-fill ota-progress" style="width:${progress}%"></div></div>
        ${device.ota_error ? `<p class="error-text device-error">${esc(device.ota_error)}</p>` : ""}
      </div>
      <button class="btn-primary-sm" data-device-deploy="${esc(device.device_id)}">Deploy OTA</button>
    </article>`;
  }).join("");
  deviceGrid.querySelectorAll("[data-device-deploy]").forEach((button) => {
    button.addEventListener("click", () => openDeployDialog(button.dataset.deviceDeploy, null));
  });
}

async function loadBuildHistory(showError = true) {
  try {
    otaBuilds = await otaRequest("/api/ota/builds");
    renderBuildHistory();
  } catch (error) {
    buildsTbody.innerHTML = `<tr><td colspan="8" class="table-empty error-text">${esc(error.message)}</td></tr>`;
    if (showError) showToast(error.message, "error", 6000);
  }
}

function renderBuildHistory() {
  if (!otaBuilds.length) {
    buildsTbody.innerHTML = '<tr><td colspan="8" class="table-empty">No firmware builds yet.</td></tr>';
    return;
  }
  buildsTbody.innerHTML = otaBuilds.map((build) => `
    <tr>
      <td>${otaDate(build.created_at)}</td>
      <td>${esc(build.project_name)}</td>
      <td>v${esc(build.version)}</td>
      <td>${esc(otaTargetLabel(build.target))}</td>
      <td><span class="session-id-chip" title="${esc(build.build_id)}">${esc(build.build_id.slice(0, 8))}</span></td>
      <td>${badge(build.status)}</td>
      <td>${build.exit_code ?? "—"}</td>
      <td><button class="btn-sm" data-build-view="${esc(build.build_id)}">View logs</button></td>
    </tr>`).join("");
  buildsTbody.querySelectorAll("[data-build-view]").forEach((button) => {
    button.addEventListener("click", async () => {
      showSection("ota-section", "ota");
      const build = await getBuildStatus(button.dataset.buildView);
      renderBuildLog(build);
      if (["queued", "building"].includes(build.status)) pollBuild(build.build_id);
    });
  });
}

function renderBuildLog(build) {
  otaBuildId.textContent = build.build_id.slice(0, 8);
  otaBuildId.title = build.build_id;
  const lines = build.logs.length ? [...build.logs] : ["Waiting for build output..."];
  if (build.hint) lines.push("", `Hint: ${build.hint}`);
  otaBuildLog.textContent = lines.join("\n");
  otaBuildLog.scrollTop = otaBuildLog.scrollHeight;
  setOtaBuildState(build.status);
}

function pollBuild(buildId) {
  clearTimeout(activeBuildPoll);
  const poll = async () => {
    try {
      const build = await getBuildStatus(buildId);
      renderBuildLog(build);
      if (["queued", "building"].includes(build.status)) {
        activeBuildPoll = setTimeout(poll, 1500);
      } else {
        otaBuildBtn.disabled = false;
        otaBuildBtn.textContent = "Upload & Build";
        await Promise.all([loadFirmware(false), loadBuildHistory(false)]);
        showToast(
          build.status === "success" ? "Firmware build completed." : "Firmware build failed. Compiler errors are in the build log.",
          build.status === "success" ? "success" : "error",
          7000,
        );
      }
    } catch (error) {
      otaBuildLog.textContent += `\nPolling error: ${error.message}`;
      otaBuildBtn.disabled = false;
      otaBuildBtn.textContent = "Upload & Build";
      setOtaBuildState("failed");
    }
  };
  poll();
}

async function openDeployDialog(deviceId, firmwareId) {
  try {
    if (!otaDevices.length || !otaFirmware.length) {
      await Promise.all([loadDevices(false), loadFirmware(false)]);
    }
    if (!otaDevices.length) throw new Error("Register an ESP32 device before deploying firmware.");
    if (!otaFirmware.length) throw new Error("Build a ready firmware image before deploying.");

    const deviceSelect = document.getElementById("deploy-device");
    const firmwareSelect = document.getElementById("deploy-firmware");
    deviceSelect.innerHTML = otaDevices.map((device) =>
      `<option value="${esc(device.device_id)}">${esc(device.name)} (${esc(device.device_id)})</option>`
    ).join("");
    firmwareSelect.innerHTML = otaFirmware.map((firmware) =>
      `<option value="${esc(firmware.firmware_id)}">v${esc(firmware.version)} — ${esc(firmware.project)} — ${esc(otaTargetLabel(firmware.target))}</option>`
    ).join("");
    if (deviceId) deviceSelect.value = deviceId;
    if (firmwareId) firmwareSelect.value = firmwareId;
    document.getElementById("deploy-overlay").classList.remove("hidden");
  } catch (error) {
    showToast(error.message, "error", 6000);
  }
}

async function submitDeployment() {
  const button = document.getElementById("deploy-submit");
  const deviceId = document.getElementById("deploy-device").value;
  const firmwareId = document.getElementById("deploy-firmware").value;
  button.disabled = true;
  try {
    const result = await deployFirmware(deviceId, firmwareId);
    document.getElementById("deploy-overlay").classList.add("hidden");
    showToast(`OTA v${result.desired_version} queued for ${result.device_id}.`, "success", 6000);
    await loadDevices(false);
  } catch (error) {
    showToast(error.message, "error", 7000);
  } finally {
    button.disabled = false;
  }
}

otaBuildBtn?.addEventListener("click", async () => {
  const file = otaProjectInput.files[0];
  const version = otaVersion.value.trim();
  if (!file || !file.name.toLowerCase().endsWith(".zip")) {
    showToast("Choose a Rust project .zip file.", "error");
    return;
  }
  if (!version) {
    showToast("Enter a firmware version.", "error");
    return;
  }

  clearTimeout(activeBuildPoll);
  otaBuildBtn.disabled = true;
  otaBuildBtn.textContent = "Uploading...";
  otaBuildLog.textContent = `Uploading ${file.name}...`;
  otaBuildId.textContent = "Uploading";
  setOtaBuildState("uploading");
  try {
    const project = await uploadFirmwareProject(file, otaTarget.value, version);
    const projectMode = project.project_type === "managed_application"
      ? `Managed function application (${project.entrypoint || "src/main.rs"}, API v${project.application_api_version || 2}; stable Wi-Fi/OTA runtime attached)`
      : "Standalone firmware (project must include its own OTA client)";
    otaBuildLog.textContent += `\nProject uploaded: ${project.project_id}\nMode: ${projectMode}\nQueuing build...`;
    setOtaBuildState("queued");
    otaBuildBtn.textContent = "Building...";
    const build = await buildFirmware(project.project_id);
    otaBuildId.textContent = build.build_id.slice(0, 8);
    pollBuild(build.build_id);
  } catch (error) {
    otaBuildLog.textContent += `\nUPLOAD / BUILD FAILED\n${error.message}`;
    setOtaBuildState("failed");
    otaBuildBtn.disabled = false;
    otaBuildBtn.textContent = "Upload & Build";
    showToast(error.message, "error", 8000);
  }
});

document.getElementById("ota-refresh")?.addEventListener("click", loadOtaDashboard);
document.getElementById("devices-refresh")?.addEventListener("click", () => loadDevices());
document.getElementById("builds-refresh")?.addEventListener("click", () => loadBuildHistory());
document.getElementById("ota-detail-close")?.addEventListener("click", () => document.getElementById("ota-detail-overlay").classList.add("hidden"));
document.getElementById("deploy-close")?.addEventListener("click", () => document.getElementById("deploy-overlay").classList.add("hidden"));
document.getElementById("deploy-cancel")?.addEventListener("click", () => document.getElementById("deploy-overlay").classList.add("hidden"));
document.getElementById("deploy-submit")?.addEventListener("click", submitDeployment);

setInterval(() => {
  checkOtaService();
  if (!document.getElementById("devices-section").classList.contains("hidden")) loadDevices(false);
  if (!document.getElementById("builds-section").classList.contains("hidden")) loadBuildHistory(false);
}, 5000);

checkOtaService();
