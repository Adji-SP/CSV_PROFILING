use crate::{
    settings::{dns_reply, valid_portal_origin, WifiConfig, PORTAL_IP},
    storage::random_token,
};
use anyhow::Result;
use embedded_svc::{
    http::{server::Request, Headers, Method},
    io::{Read, Write},
};
use esp_idf_svc::{
    handle::RawHandle,
    http::server::{Configuration, EspHttpConnection, EspHttpServer},
    sys,
};
use serde::Serialize;
use std::{
    net::UdpSocket,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::SyncSender,
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

pub enum Command {
    Connect(WifiConfig),
    Scan,
    Retry,
}

#[derive(Clone, Serialize)]
pub struct VisibleNetwork {
    pub ssid: String,
    pub rssi: i8,
    pub secured: bool,
}

#[derive(Serialize)]
pub struct PortalState {
    pub ap_name: String,
    pub phase: String,
    pub message: String,
    pub saved_ssid: Option<String>,
    pub target_ssid: Option<String>,
    pub ip_address: Option<String>,
    pub busy: bool,
    pub scanning: bool,
    pub scan_error: Option<String>,
    pub close_in: Option<u64>,
    pub networks: Vec<VisibleNetwork>,
    pub csrf: String,
}
pub type SharedState = Arc<Mutex<PortalState>>;

impl PortalState {
    pub fn new(ap_name: &str, saved: Option<&WifiConfig>) -> Self {
        Self {
            ap_name: ap_name.into(),
            phase: "idle".into(),
            message: "Choose a 2.4 GHz network to get this board online.".into(),
            saved_ssid: saved.map(|c| c.ssid.clone()),
            target_ssid: None,
            ip_address: None,
            busy: false,
            scanning: false,
            scan_error: None,
            close_in: None,
            networks: vec![],
            csrf: String::new(),
        }
    }
}

pub struct Portal {
    _http: EspHttpServer<'static>,
    stop: Arc<AtomicBool>,
    dns: Option<JoinHandle<()>>,
}

impl Portal {
    pub fn start(state: SharedState, tx: SyncSender<Command>) -> Result<Self> {
        state.lock().unwrap().csrf = random_token();
        let mut http = EspHttpServer::new(&Configuration {
            stack_size: 10 * 1024,
            max_open_sockets: 3,
            max_uri_handlers: 12,
            max_resp_headers: 10,
            uri_match_wildcard: true,
            ..Default::default()
        })?;
        for (path, content_type, body) in [
            (
                "/",
                "text/html; charset=utf-8",
                include_bytes!("../web/index.html").as_slice(),
            ),
            (
                "/status",
                "text/html; charset=utf-8",
                include_bytes!("../web/status.html").as_slice(),
            ),
            (
                "/setup.css",
                "text/css",
                include_bytes!("../web/setup.css").as_slice(),
            ),
            (
                "/setup.js",
                "text/javascript",
                include_bytes!("../web/setup.js").as_slice(),
            ),
        ] {
            http.fn_handler::<anyhow::Error, _>(path, Method::Get, move |mut req| {
                if !ap_request(&mut req) {
                    return reply(
                        req,
                        403,
                        "text/plain",
                        b"Join the board's setup Wi-Fi first.",
                    );
                }
                if !valid_portal_origin(req.header("Host"), req.header("Origin")) {
                    return redirect(req);
                }
                reply(req, 200, content_type, body)
            })?;
        }
        let view = state.clone();
        http.fn_handler::<anyhow::Error, _>("/api/status", Method::Get, move |mut req| {
            if !ap_request(&mut req)
                || !valid_portal_origin(req.header("Host"), req.header("Origin"))
            {
                return json_error(
                    req,
                    403,
                    "Join the board's setup Wi-Fi and open http://192.168.4.1.",
                );
            }
            let body = serde_json::to_vec(&*view.lock().unwrap())?;
            reply(req, 200, "application/json", &body)
        })?;
        for path in ["/api/connect", "/api/scan", "/api/retry"] {
            let shared = state.clone();
            let commands = tx.clone();
            http.fn_handler::<anyhow::Error, _>(path, Method::Post, move |mut req| {
                if !ap_request(&mut req)
                    || !valid_portal_origin(req.header("Host"), req.header("Origin"))
                {
                    return json_error(req, 403, "Setup is only available on the board's hotspot.");
                }
                {
                    let view = shared.lock().unwrap();
                    if req.header("X-Setup-Token") != Some(view.csrf.as_str()) {
                        return json_error(req, 403, "Setup session expired. Reload this page.");
                    }
                }
                if !req
                    .header("Content-Type")
                    .is_some_and(|s| s.split(';').next() == Some("application/json"))
                {
                    return json_error(req, 415, "Send a JSON request.");
                }
                let len = match req.content_len() {
                    Some(len) if len > 0 && len <= 512 => len as usize,
                    _ => return json_error(req, 413, "Request must contain 1-512 bytes."),
                };
                let mut body = vec![0u8; len];
                req.read_exact(&mut body)?;
                let command = if path == "/api/connect" {
                    let config: WifiConfig = match serde_json::from_slice(&body) {
                        Ok(config) => config,
                        Err(_) => return json_error(req, 400, "Enter a Wi-Fi name and password."),
                    };
                    if let Err(message) = config.validate() {
                        return json_error(req, 400, message);
                    }
                    Command::Connect(config)
                } else if path == "/api/scan" {
                    Command::Scan
                } else {
                    Command::Retry
                };
                {
                    let mut view = shared.lock().unwrap();
                    if view.busy || view.scanning {
                        return json_error(
                            req,
                            409,
                            "An operation is already running. Please wait.",
                        );
                    }
                    if path == "/api/retry" && view.saved_ssid.is_none() {
                        return json_error(req, 400, "No saved network yet.");
                    }
                    if commands.try_send(command).is_err() {
                        return json_error(req, 503, "Wi-Fi manager is busy. Please retry.");
                    }
                    if path == "/api/scan" {
                        view.scanning = true;
                    } else {
                        view.busy = true;
                        view.phase = "connecting".into();
                        view.message =
                            "Connecting to Wi-Fi and waiting for an IP address...".into();
                        view.close_in = None;
                    }
                }
                reply(req, 202, "application/json", b"{\"accepted\":true}")
            })?;
        }
        // Android/Apple/Windows captive checks and unknown HTTP paths lead here.
        // Never intercept HTTPS or return success to connectivity probes.
        http.fn_handler::<anyhow::Error, _>("/*", Method::Get, |mut req| {
            if !ap_request(&mut req) {
                return reply(req, 403, "text/plain", b"Setup hotspot only.");
            }
            redirect(req)
        })?;
        let socket = UdpSocket::bind((PORTAL_IP, 53))?;
        socket.set_read_timeout(Some(Duration::from_millis(150)))?;
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let dns = thread::Builder::new()
            .name("setup-dns".into())
            .stack_size(4096)
            .spawn(move || {
                let mut buffer = [0u8; 512];
                while !flag.load(Ordering::Relaxed) {
                    if let Ok((len, peer)) = socket.recv_from(&mut buffer) {
                        if let Some(answer) = dns_reply(&buffer[..len]) {
                            let _ = socket.send_to(&answer, peer);
                        }
                    }
                }
            })?;
        Ok(Self {
            _http: http,
            stop,
            dns: Some(dns),
        })
    }
}

impl Drop for Portal {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(task) = self.dns.take() {
            let _ = task.join();
        }
    }
}

/// Check the actual local socket address, not just the forgeable Host header.
/// The HTTP server listens on both netifs; station-side access is denied.
fn ap_request(req: &mut Request<&mut EspHttpConnection<'_>>) -> bool {
    let Ok(raw) = req.connection().raw_connection() else {
        return false;
    };
    let Ok(source) = raw.source_ipv4() else {
        return false;
    };
    if source.octets()[..3] != [192, 168, 4] {
        return false;
    }
    unsafe {
        let fd = sys::httpd_req_to_sockfd(raw.handle());
        if fd < 0 {
            return false;
        }
        let mut address: sys::sockaddr_in = std::mem::zeroed();
        let mut len = std::mem::size_of::<sys::sockaddr_in>() as sys::socklen_t;
        let result =
            sys::lwip_getsockname(fd, &mut address as *mut _ as *mut sys::sockaddr, &mut len);
        result == 0
            && i32::from(address.sin_family) == sys::AF_INET as i32
            && u32::from_be(address.sin_addr.s_addr) == u32::from_be_bytes([192, 168, 4, 1])
    }
}

fn reply(
    req: Request<&mut EspHttpConnection<'_>>,
    status: u16,
    content_type: &str,
    body: &[u8],
) -> Result<()> {
    req.into_response(status, None, &[
        ("Content-Type", content_type), ("Cache-Control", "no-store"),
        ("X-Content-Type-Options", "nosniff"), ("Referrer-Policy", "no-referrer"),
        ("Content-Security-Policy", "default-src 'self'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self' data:; frame-ancestors 'none'; base-uri 'none'; form-action 'self'"),
        ("Connection", "close"),
    ])?.write_all(body)?;
    Ok(())
}
fn json_error(req: Request<&mut EspHttpConnection<'_>>, status: u16, message: &str) -> Result<()> {
    reply(
        req,
        status,
        "application/json",
        &serde_json::to_vec(&serde_json::json!({"error":message}))?,
    )
}
fn redirect(req: Request<&mut EspHttpConnection<'_>>) -> Result<()> {
    req.into_response(
        302,
        None,
        &[
            ("Location", "http://192.168.4.1/"),
            ("Cache-Control", "no-store"),
            ("Connection", "close"),
        ],
    )?
    .write_all(b"Open http://192.168.4.1/")?;
    Ok(())
}
