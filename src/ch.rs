//! Minimal client for the cloud-hypervisor REST API over its unix socket.

use std::collections::BTreeMap;
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

const TIMEOUT: Duration = Duration::from_secs(5);

pub struct Client {
    socket: PathBuf,
}

#[derive(Debug, PartialEq)]
pub struct Response {
    pub status: u16,
    pub body: String,
}

/// Why the API could not be reached, so callers can degrade gracefully
/// (e.g. `list` without root) instead of failing outright.
#[derive(Debug, PartialEq)]
pub enum Unavailable {
    NotRunning,
    PermissionDenied,
}

impl Client {
    pub fn new(socket: impl Into<PathBuf>) -> Self {
        Self {
            socket: socket.into(),
        }
    }

    pub fn probe(socket: &Path) -> std::result::Result<Self, Unavailable> {
        match UnixStream::connect(socket) {
            Ok(_) => Ok(Self::new(socket)),
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                Err(Unavailable::PermissionDenied)
            }
            Err(_) => Err(Unavailable::NotRunning),
        }
    }

    pub fn request(&self, method: &str, endpoint: &str, body: Option<&str>) -> Result<Response> {
        let mut stream = UnixStream::connect(&self.socket).map_err(|e| match e.kind() {
            io::ErrorKind::PermissionDenied => anyhow!(
                "permission denied on {}: the API socket is owner-only, run as root (sudo microvmctl ...)",
                self.socket.display()
            ),
            io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused => {
                anyhow!("VM is not running (no API at {})", self.socket.display())
            }
            _ => anyhow!(e).context(format!("connecting to {}", self.socket.display())),
        })?;
        stream.set_read_timeout(Some(TIMEOUT))?;
        stream.set_write_timeout(Some(TIMEOUT))?;

        let endpoint = endpoint.trim_start_matches('/');
        let body = body.unwrap_or("");
        let request = format!(
            "{method} /api/v1/{endpoint} HTTP/1.1\r\nHost: localhost\r\nAccept: application/json\r\n\
             Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(request.as_bytes())?;
        read_response(BufReader::new(stream)).context("reading API response")
    }

    pub fn get<T: DeserializeOwned>(&self, endpoint: &str) -> Result<T> {
        let resp = self.checked("GET", endpoint, None)?;
        serde_json::from_str(&resp.body).with_context(|| format!("decoding {endpoint} response"))
    }

    pub fn put(&self, endpoint: &str, body: Option<&str>) -> Result<()> {
        self.checked("PUT", endpoint, body).map(drop)
    }

    fn checked(&self, method: &str, endpoint: &str, body: Option<&str>) -> Result<Response> {
        let resp = self.request(method, endpoint, body)?;
        if !(200..300).contains(&resp.status) {
            bail!(
                "{method} {endpoint}: HTTP {}: {}",
                resp.status,
                error_message(&resp.body)
            );
        }
        Ok(resp)
    }
}

/// cloud-hypervisor reports errors as a JSON array of the error chain,
/// outermost first, e.g. `["Error from API","The VM could not be paused",...]`.
fn error_message(body: &str) -> String {
    match serde_json::from_str::<Vec<String>>(body) {
        Ok(chain) => chain
            .into_iter()
            .filter(|e| e != "Error from API")
            .collect::<Vec<_>>()
            .join(": "),
        Err(_) => body.trim().to_string(),
    }
}

/// cloud-hypervisor answers with `Connection: keep-alive` and never closes
/// the socket, so the body must be delimited by `Content-Length`, not EOF.
fn read_response(mut reader: impl BufRead) -> Result<Response> {
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let status = line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| anyhow!("malformed status line {line:?}"))?;

    let mut content_length = 0;
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        let header = line.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            content_length = value.trim().parse().context("bad Content-Length")?;
        }
    }

    let mut body = vec![0; content_length];
    reader.read_exact(&mut body)?;
    Ok(Response {
        status,
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}

// Partial views of the API types: only what we display. Everything is
// defaulted so newer cloud-hypervisor versions with extra or missing fields
// still decode.

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct VmInfo {
    pub state: String,
    pub memory_actual_size: Option<u64>,
    pub config: VmConfig,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct VmConfig {
    pub cpus: Cpus,
    pub memory: Memory,
    pub disks: Option<Vec<Disk>>,
    pub net: Option<Vec<Net>>,
    pub fs: Option<Vec<Fs>>,
    pub balloon: Option<Balloon>,
    pub console: Console,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Cpus {
    pub boot_vcpus: u32,
    pub max_vcpus: u32,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Memory {
    pub size: u64,
    pub shared: bool,
    pub mergeable: bool,
    pub hugepages: bool,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Disk {
    pub id: Option<String>,
    pub path: Option<PathBuf>,
    pub readonly: bool,
    pub direct: bool,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Net {
    pub id: Option<String>,
    pub tap: Option<String>,
    pub mac: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Fs {
    pub id: Option<String>,
    pub tag: String,
    pub socket: PathBuf,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Balloon {
    pub size: u64,
}

/// `mode` is `Off`, `Null`, `Tty`, `File`, `Pty` or `Socket`. In `Pty`
/// mode cloud-hypervisor fills `file` with the allocated `/dev/pts/N`.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Console {
    pub mode: String,
    pub file: Option<PathBuf>,
}

/// `vm.counters`: device id -> counter name -> value.
pub type Counters = BTreeMap<String, BTreeMap<String, u64>>;

#[derive(Debug, Default, Clone, Copy, PartialEq, Serialize)]
pub struct IoTotals {
    pub disk_read: u64,
    pub disk_write: u64,
    pub net_rx: u64,
    pub net_tx: u64,
}

pub fn io_totals(counters: &Counters) -> IoTotals {
    let mut t = IoTotals::default();
    for c in counters.values() {
        let get = |k: &str| c.get(k).copied().unwrap_or(0);
        t.disk_read += get("read_bytes");
        t.disk_write += get("write_bytes");
        t.net_rx += get("rx_bytes");
        t.net_tx += get("tx_bytes");
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_body_by_content_length_without_eof() {
        // Trailing bytes stand in for a kept-alive connection that never closes.
        let raw = "HTTP/1.1 200 \r\nServer: Cloud Hypervisor API\r\nConnection: keep-alive\r\n\
                   Content-Type: application/json\r\nContent-Length: 18\r\n\r\n{\"state\":\"Paused\"}GARBAGE";
        let resp = read_response(raw.as_bytes()).unwrap();
        assert_eq!(resp.status, 200);
        assert_eq!(resp.body, "{\"state\":\"Paused\"}");
    }

    #[test]
    fn flattens_error_chain() {
        let body = r#"["Error from API","The VM could not be paused","Cannot pause VM"]"#;
        assert_eq!(
            error_message(body),
            "The VM could not be paused: Cannot pause VM"
        );
        assert_eq!(error_message("plain failure\n"), "plain failure");
    }

    #[test]
    fn reads_empty_204() {
        let resp = read_response("HTTP/1.1 204 \r\nServer: x\r\n\r\n".as_bytes()).unwrap();
        assert_eq!(resp.status, 204);
        assert_eq!(resp.body, "");
    }

    #[test]
    fn decodes_vm_info_subset() {
        let json = r#"{"state":"Running","memory_actual_size":2147483648,"device_tree":{},
            "config":{"cpus":{"boot_vcpus":2,"max_vcpus":2,"nested":true},
            "memory":{"size":2147483648,"mergeable":true,"shared":false,"hugepages":false},
            "disks":[{"id":"_disk0","path":"var.img","readonly":false,"direct":false,"num_queues":2}],
            "net":[{"id":"_net3","tap":"vm-osprey","mac":"02:00:00:00:00:02","ip":null}],
            "fs":null,"balloon":null}}"#;
        let info: VmInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.state, "Running");
        assert_eq!(info.config.cpus.boot_vcpus, 2);
        assert!(info.config.memory.mergeable);
        assert_eq!(info.config.disks.unwrap()[0].path, Some("var.img".into()));
        assert_eq!(
            info.config.net.unwrap()[0].tap.as_deref(),
            Some("vm-osprey")
        );
        assert!(info.config.fs.is_none());
    }

    #[test]
    fn sums_io_counters() {
        let counters: Counters = serde_json::from_str(
            r#"{"_disk0":{"read_bytes":10,"write_bytes":20,"read_ops":1},
                "_disk1":{"read_bytes":1,"write_bytes":2},
                "_net2":{"rx_bytes":100,"tx_bytes":200}}"#,
        )
        .unwrap();
        assert_eq!(
            io_totals(&counters),
            IoTotals {
                disk_read: 11,
                disk_write: 22,
                net_rx: 100,
                net_tx: 200
            }
        );
    }
}
