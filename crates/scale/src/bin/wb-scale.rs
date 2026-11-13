//! Installer / diagnostics CLI.
//!
//!   wb-scale ports
//!   wb-scale detect                      # scan all serial ports
//!   wb-scale detect --tcp 10.0.0.50:4001
//!   wb-scale monitor --tcp 127.0.0.1:4001 --profile toledo
//!   wb-scale learn --tcp 127.0.0.1:4001 --weight 12340
//!   wb-scale profiles

use clap::{Parser, Subcommand};
use std::time::Duration;
use wb_scale::{detect, find_profile, Transport};

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(clap::Args, Clone)]
struct LinkArgs {
    #[arg(long)]
    tcp: Option<String>,
    #[arg(long)]
    serial: Option<String>,
    #[arg(long, default_value_t = 9600)]
    baud: u32,
    #[arg(long, default_value = "8N1")]
    framing: String,
}

impl LinkArgs {
    fn transport(&self) -> anyhow::Result<Transport> {
        if let Some(t) = &self.tcp {
            let (h, p) = t.rsplit_once(':').ok_or_else(|| anyhow::anyhow!("use host:port"))?;
            return Ok(Transport::Tcp { host: h.into(), port: p.parse()? });
        }
        let port = self.serial.clone().ok_or_else(|| anyhow::anyhow!("give --tcp or --serial"))?;
        Ok(Transport::Serial { port, baud: self.baud, framing: self.framing.clone() })
    }
}

#[derive(Subcommand)]
enum Cmd {
    /// List serial ports
    Ports,
    /// List built-in protocol profiles
    Profiles,
    /// Find the port, line settings and protocol automatically
    Detect {
        #[arg(long)]
        tcp: Option<String>,
        /// Only scan these serial ports
        #[arg(long)]
        port: Vec<String>,
    },
    /// Print live readings
    Monitor {
        #[command(flatten)]
        link: LinkArgs,
        #[arg(long, default_value = "generic-number")]
        profile: String,
    },
    /// Build a profile for an unknown indicator from the weight on its display
    Learn {
        #[command(flatten)]
        link: LinkArgs,
        #[arg(long)]
        weight: f64,
    },
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter("warn").init();
    match Cli::parse().cmd {
        Cmd::Ports => {
            for p in wb_scale::transport::list_serial_ports() {
                println!("{p}");
            }
        }
        Cmd::Profiles => {
            for p in wb_scale::builtin_profiles() {
                println!("{:<16} {}\n{:<16} {}\n", p.id, p.name, "", p.description);
            }
        }
        Cmd::Detect { tcp, port } => {
            let cands = if let Some(t) = tcp {
                let (h, p) = t.rsplit_once(':').ok_or_else(|| anyhow::anyhow!("use host:port"))?;
                detect::detect_tcp(h, p.parse()?, &[], Duration::from_secs(2))?
            } else {
                let ports = if port.is_empty() { wb_scale::transport::list_serial_ports() } else { port };
                detect::detect_serial(&ports, &[], Duration::from_millis(1200), |s| eprintln!("  {s}"))
            };
            if cands.is_empty() {
                println!("Nothing recognised. Check the cable/output mode, or run `wb-scale learn`.");
            }
            for c in cands {
                println!(
                    "{:.2}  {}  profile={}  weights={:?}  stable_flag={}\n      raw: {}",
                    c.score, c.transport.label(), c.profile.id, c.sample_weights, c.stable_flag_seen, c.raw_sample
                );
            }
        }
        Cmd::Monitor { link, profile } => {
            let profile = find_profile(&profile, &[]).ok_or_else(|| anyhow::anyhow!("unknown profile"))?;
            let h = wb_scale::start(wb_scale::ScaleConfig { transport: link.transport()?, profile, stability: Default::default() })?;
            loop {
                std::thread::sleep(Duration::from_millis(500));
                let s = h.current();
                match (&s.reading, s.receiving) {
                    (Some(r), true) => println!("{:>10.0} kg  {}  raw={}", r.weight_kg, if r.stable { "STABLE" } else { "motion" }, r.raw),
                    _ => println!("no data  connected={} err={:?}", s.connected, s.error),
                }
            }
        }
        Cmd::Learn { link, weight } => {
            let bytes = detect::capture(&link.transport()?, Duration::from_secs(2), None)?;
            match detect::learn(&bytes, weight) {
                Some(p) => println!("{}", serde_json::to_string_pretty(&p)?),
                None => println!("Could not find {weight} in the data. Raw: {}", wb_scale::profile::escape(&bytes[..bytes.len().min(200)])),
            }
        }
    }
    Ok(())
}
