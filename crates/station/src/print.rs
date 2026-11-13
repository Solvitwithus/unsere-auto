//! Ticket printing: ESC/POS to network, serial or file printers, or a command.

use crate::config::PrinterConfig;
use std::io::Write;
use std::time::Duration;

pub fn escpos(text: &str) -> Vec<u8> {
    let mut b = vec![0x1b, b'@']; // init
    for line in text.lines() {
        b.extend_from_slice(line.as_bytes());
        b.extend_from_slice(b"\r\n");
    }
    b.extend_from_slice(b"\r\n\r\n\r\n\r\n");
    b.extend_from_slice(&[0x1d, b'V', 0x01]); // partial cut
    b
}

/// Ok(false) = browser printing (the UI prints the HTML ticket itself).
pub fn print(cfg: &PrinterConfig, text: &str) -> anyhow::Result<bool> {
    match cfg {
        PrinterConfig::Browser => Ok(false),
        PrinterConfig::Tcp { host, port } => {
            let addr = std::net::ToSocketAddrs::to_socket_addrs(&(host.as_str(), *port))?.next().ok_or_else(|| anyhow::anyhow!("cannot resolve {host}"))?;
            let mut s = std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(4))?;
            s.write_all(&escpos(text))?;
            Ok(true)
        }
        PrinterConfig::Serial { port, baud } => {
            let t = wb_scale::Transport::Serial { port: port.clone(), baud: *baud, framing: "8N1".into() };
            let mut l = t.open(Duration::from_millis(500))?;
            l.write_all(&escpos(text))?;
            l.flush()?;
            Ok(true)
        }
        PrinterConfig::File { path } => {
            std::fs::OpenOptions::new().create(true).append(true).open(path)?.write_all(&escpos(text))?;
            Ok(true)
        }
        PrinterConfig::Command { command } => {
            let mut parts = command.split_whitespace();
            let prog = parts.next().ok_or_else(|| anyhow::anyhow!("empty print command"))?;
            let mut child = std::process::Command::new(prog).args(parts).stdin(std::process::Stdio::piped()).spawn()?;
            child.stdin.take().unwrap().write_all(text.as_bytes())?;
            let st = child.wait()?;
            anyhow::ensure!(st.success(), "print command failed: {st}");
            Ok(true)
        }
    }
}
