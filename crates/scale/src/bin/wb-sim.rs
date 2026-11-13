//! Weighbridge indicator simulator. Plays a loop of trucks driving on, settling,
//! and driving off, and streams it in a real indicator format.
//!
//!   wb-sim --format toledo --tcp 127.0.0.1:4001
//!   wb-sim --format st-gs --serial /dev/pts/5 --baud 2400

use clap::Parser;
use std::io::Write;
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Parser)]
struct Args {
    /// toledo | st-gs | yaohua | weight-unit | sign6
    #[arg(long, default_value = "toledo")]
    format: String,
    /// Serve frames to TCP clients on this address
    #[arg(long)]
    tcp: Option<String>,
    /// Write frames to this serial device (e.g. a socat pty)
    #[arg(long)]
    serial: Option<String>,
    #[arg(long, default_value_t = 9600)]
    baud: u32,
    #[arg(long, default_value = "8N1")]
    framing: String,
    /// Frames per second
    #[arg(long, default_value_t = 10)]
    rate: u32,
    /// Hold a constant weight instead of the truck scenario
    #[arg(long)]
    hold: Option<f64>,
    /// Scale division in kg
    #[arg(long, default_value_t = 20.0)]
    division: f64,
}

#[derive(Clone, Copy)]
struct Live {
    kg: f64,
    motion: bool,
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % 1_000_000) as f64 / 1_000_000.0
    }
}

fn encode(format: &str, l: Live) -> Vec<u8> {
    let w = l.kg.round() as i64;
    let a = w.unsigned_abs();
    match format {
        "toledo" => {
            let swb = 0x20 | if w < 0 { 0x02 } else { 0 } | if l.motion { 0x08 } else { 0 };
            let mut f = vec![0x02, 0x22, swb, 0x20];
            f.extend_from_slice(format!("{a:06}000000").as_bytes());
            f.push(b'\r');
            let sum: u32 = f.iter().map(|b| *b as u32).sum();
            f.push(((!sum + 1) & 0x7f) as u8);
            f
        }
        "st-gs" => format!("{},GS,{}{a:07}kg\r\n", if l.motion { "US" } else { "ST" }, if w < 0 { '-' } else { '+' }).into_bytes(),
        "yaohua" => {
            let d: String = format!("{a:07}").chars().rev().collect();
            format!("={}{d}", if w < 0 { "-" } else { "" }).into_bytes()
        }
        "weight-unit" => format!("{w:>8} kg G{}\r\n", if l.motion { " M" } else { "" }).into_bytes(),
        "sign6" => format!("{}{a:06}00\r\n", if w < 0 { '-' } else { '+' }).into_bytes(),
        other => panic!("unknown format {other}"),
    }
}

fn scenario(live: Arc<Mutex<Live>>, hold: Option<f64>, div: f64) {
    let mut rng = Rng(Instant::now().elapsed().as_nanos() as u64 | 0x9E3779B97F4A7C15);
    let q = |v: f64| (v / div).round() * div;
    let set = |kg: f64, motion: bool| *live.lock().unwrap() = Live { kg, motion };
    if let Some(h) = hold {
        set(q(h), false);
        loop {
            std::thread::sleep(Duration::from_secs(3600));
        }
    }
    let tick = Duration::from_millis(100);
    loop {
        // empty deck
        for _ in 0..50 {
            set(0.0, false);
            std::thread::sleep(tick);
        }
        let target = q(8000.0 + rng.next() * 34000.0);
        // driving on: ramp with bounce
        for i in 0..40 {
            let f = (i as f64 / 40.0).min(1.0);
            let noise = (rng.next() - 0.5) * 600.0 * (1.0 - f);
            set(q(target * f + noise), true);
            std::thread::sleep(tick);
        }
        // settling
        for _ in 0..15 {
            set(q(target + (rng.next() - 0.5) * 80.0), true);
            std::thread::sleep(tick);
        }
        // stable on the deck
        for _ in 0..200 {
            set(target, false);
            std::thread::sleep(tick);
        }
        // driving off
        for i in 0..30 {
            set(q(target * (1.0 - i as f64 / 30.0)), true);
            std::thread::sleep(tick);
        }
    }
}

fn main() -> anyhow::Result<()> {
    let a = Args::parse();
    encode(&a.format, Live { kg: 0.0, motion: false }); // validate format early
    let live = Arc::new(Mutex::new(Live { kg: 0.0, motion: false }));
    {
        let live = live.clone();
        let (hold, div) = (a.hold, a.division);
        std::thread::spawn(move || scenario(live, hold, div));
    }
    let period = Duration::from_millis(1000 / a.rate.max(1) as u64);

    if let Some(dev) = &a.serial {
        let t = wb_scale::Transport::Serial { port: dev.clone(), baud: a.baud, framing: a.framing.clone() };
        let mut link = t.open(Duration::from_millis(100))?;
        eprintln!("wb-sim: {} frames -> {}", a.format, t.label());
        loop {
            let l = *live.lock().unwrap();
            link.write_all(&encode(&a.format, l))?;
            std::thread::sleep(period);
        }
    }

    let addr = a.tcp.clone().unwrap_or_else(|| "127.0.0.1:4001".into());
    let listener = TcpListener::bind(&addr)?;
    eprintln!("wb-sim: {} frames on tcp://{addr}", a.format);
    for stream in listener.incoming() {
        let Ok(mut s) = stream else { continue };
        let (live, fmt) = (live.clone(), a.format.clone());
        std::thread::spawn(move || loop {
            let l = *live.lock().unwrap();
            if s.write_all(&encode(&fmt, l)).is_err() {
                break;
            }
            std::thread::sleep(period);
        });
    }
    Ok(())
}
