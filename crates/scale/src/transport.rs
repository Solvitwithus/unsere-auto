//! Physical links to the indicator: RS-232/RS-485 (incl. USB adapters) and TCP/IP.

use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Transport {
    Serial {
        port: String,
        #[serde(default = "d_baud")]
        baud: u32,
        /// e.g. "8N1", "7E1", "8E1", "7O1", "8N2"
        #[serde(default = "d_framing")]
        framing: String,
    },
    Tcp { host: String, port: u16 },
}

fn d_baud() -> u32 {
    9600
}
fn d_framing() -> String {
    "8N1".into()
}

pub const COMMON_BAUDS: &[u32] = &[9600, 2400, 4800, 1200, 19200];
pub const COMMON_FRAMINGS: &[&str] = &["8N1", "7E1"];

pub trait Link: Read + Write + Send {}
impl<T: Read + Write + Send> Link for T {}

impl Transport {
    pub fn label(&self) -> String {
        match self {
            Transport::Serial { port, baud, framing } => format!("{port} @ {baud} {framing}"),
            Transport::Tcp { host, port } => format!("tcp://{host}:{port}"),
        }
    }

    pub fn open(&self, read_timeout: Duration) -> anyhow::Result<Box<dyn Link>> {
        match self {
            Transport::Serial { port, baud, framing } => {
                let (db, parity, sb) = parse_framing(framing)?;
                let mut p = serialport::new(port, *baud)
                    .data_bits(db)
                    .parity(parity)
                    .stop_bits(sb)
                    .flow_control(serialport::FlowControl::None)
                    .timeout(read_timeout)
                    .open()
                    .map_err(|e| anyhow::anyhow!("cannot open {port}: {e}"))?;
                // Some indicators misbehave when DTR/RTS are asserted (the old app cleared both).
                let _ = p.write_data_terminal_ready(false);
                let _ = p.write_request_to_send(false);
                Ok(Box::new(p))
            }
            Transport::Tcp { host, port } => {
                let addr = (host.as_str(), *port)
                    .to_socket_addrs()?
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("cannot resolve {host}"))?;
                let s = TcpStream::connect_timeout(&addr, Duration::from_secs(3))?;
                s.set_read_timeout(Some(read_timeout))?;
                s.set_nodelay(true)?;
                Ok(Box::new(s))
            }
        }
    }
}

pub fn parse_framing(f: &str) -> anyhow::Result<(serialport::DataBits, serialport::Parity, serialport::StopBits)> {
    use serialport::{DataBits, Parity, StopBits};
    let b = f.trim().to_ascii_uppercase();
    let b = b.as_bytes();
    anyhow::ensure!(b.len() == 3, "framing must look like 8N1");
    let db = match b[0] {
        b'5' => DataBits::Five,
        b'6' => DataBits::Six,
        b'7' => DataBits::Seven,
        b'8' => DataBits::Eight,
        _ => anyhow::bail!("bad data bits in {f}"),
    };
    let parity = match b[1] {
        b'N' => Parity::None,
        b'E' => Parity::Even,
        b'O' => Parity::Odd,
        _ => anyhow::bail!("bad parity in {f}"),
    };
    let sb = match b[2] {
        b'1' => StopBits::One,
        b'2' => StopBits::Two,
        _ => anyhow::bail!("bad stop bits in {f}"),
    };
    Ok((db, parity, sb))
}

/// Serial ports on this machine. Works without libudev: falls back to scanning /dev.
pub fn list_serial_ports() -> Vec<String> {
    let mut out: Vec<String> = serialport::available_ports()
        .map(|v| v.into_iter().map(|p| p.port_name).collect())
        .unwrap_or_default();
    #[cfg(unix)]
    if let Ok(rd) = std::fs::read_dir("/dev") {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            let wanted = n.starts_with("ttyUSB") || n.starts_with("ttyACM") || n.starts_with("ttyAMA")
                || matches!(n.as_str(), "ttyS0" | "ttyS1" | "ttyS2" | "ttyS3")
                || n.starts_with("cu.usbserial") || n.starts_with("weighbridge");
            let path = format!("/dev/{n}");
            if wanted && !out.contains(&path) {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}
