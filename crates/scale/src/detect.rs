//! Auto-detect: listen on each port at the common line settings and see which
//! profile makes sense of the bytes. Also: Learn mode for unknown indicators.

use crate::profile::{builtin_profiles, escape, Format, Profile};
use crate::transport::{Transport, COMMON_BAUDS, COMMON_FRAMINGS};
use regex::Regex;
use serde::Serialize;
use std::io::{ErrorKind, Read, Write};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Serialize)]
pub struct Candidate {
    pub transport: Transport,
    pub profile: Profile,
    /// Distinct weights seen during the capture, for the installer to compare
    /// with the indicator display ("which of these matches?").
    pub sample_weights: Vec<f64>,
    pub stable_flag_seen: bool,
    pub score: f64,
    pub raw_sample: String,
}

/// Read raw bytes from a link for `dur`. Sends `poll` once first, if given.
pub fn capture(t: &Transport, dur: Duration, poll: Option<&[u8]>) -> anyhow::Result<Vec<u8>> {
    let mut link = t.open(Duration::from_millis(100))?;
    if let Some(p) = poll {
        let _ = link.write_all(p);
    }
    let mut out = Vec::new();
    let mut buf = [0u8; 512];
    let until = Instant::now() + dur;
    while Instant::now() < until && out.len() < 16 * 1024 {
        match link.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => out.extend_from_slice(&buf[..n]),
            Err(e) if matches!(e.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock | ErrorKind::Interrupted) => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(out)
}

fn specificity(id: &str) -> f64 {
    match id {
        "toledo" => 1.0,
        "st-gs" => 0.97,
        "weight-unit" => 0.93,
        "yaohua-reversed" => 0.9,
        "sign6" => 0.85,
        "generic-number" => 0.5,
        _ => 0.95, // custom / learned profiles are trusted
    }
}

/// Score every profile against captured bytes. Highest score first.
pub fn evaluate(bytes: &[u8], profiles: &[Profile]) -> Vec<(Profile, f64, Vec<f64>, bool)> {
    // At the wrong baud rate a serial port receives garbage bytes. Text
    // protocols are almost 100% printable at the right one, so a low
    // printable ratio rules out every text profile straight away.
    let printable = if bytes.is_empty() {
        0.0
    } else {
        bytes.iter().filter(|b| (0x20..0x7f).contains(*b) || matches!(b, b'\r' | b'\n' | 0x02 | 0x03)).count() as f64
            / bytes.len() as f64
    };
    let mut out = Vec::new();
    for p in profiles {
        if p.reverse_digits && !bytes.contains(&b'=') {
            continue;
        }
        if p.format == Format::Regex && printable < 0.9 {
            continue; // wrong baud rate produces garbage
        }
        let Ok(c) = p.compile() else { continue };
        let (frames, _) = c.split(bytes);
        // ignore 1-2 byte fragments (checksums, partial frames)
        let frames: Vec<_> = frames.into_iter().filter(|f| f.len() >= 3).collect();
        if frames.len() < 3 {
            continue;
        }
        let parsed: Vec<_> = frames.iter().filter_map(|f| c.parse(f)).filter(|r| r.weight_kg.abs() < 500_000.0).collect();
        if parsed.len() < 3 {
            continue;
        }
        // Score = share of frames this profile understood, weighted by how
        // specific the profile is (a generic "first number" profile matches
        // almost anything, so it only wins when nothing better does).
        let ratio = parsed.len() as f64 / frames.len() as f64;
        let mut weights: Vec<f64> = parsed.iter().map(|r| r.weight_kg).collect();
        weights.dedup();
        weights.truncate(6);
        let flag = parsed.iter().any(|r| r.stable.is_some());
        let score = ratio * specificity(&p.id) * if flag { 1.0 } else { 0.97 };
        out.push((p.clone(), score, weights, flag));
    }
    out.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    out
}

/// Try every given serial port at the common baud rates / framings.
/// `on_progress` is called with a human-readable status line.
pub fn detect_serial(ports: &[String], extra: &[Profile], per_try: Duration, mut on_progress: impl FnMut(&str)) -> Vec<Candidate> {
    let mut profiles: Vec<Profile> = extra.to_vec();
    profiles.extend(builtin_profiles());
    let mut found = Vec::new();
    // Try each port at each common line setting. Listening takes ~1.2 s per
    // combination; we stop early on a port once a confident match is found.
    for port in ports {
        let mut best_for_port = 0.0f64;
        'tries: for framing in COMMON_FRAMINGS {
            for baud in COMMON_BAUDS {
                let t = Transport::Serial { port: port.clone(), baud: *baud, framing: framing.to_string() };
                on_progress(&format!("listening on {}", t.label()));
                let bytes = match capture(&t, per_try, None) {
                    Ok(b) => b,
                    Err(e) => {
                        on_progress(&format!("{port}: {e}"));
                        break 'tries; // port can't be opened at all
                    }
                };
                if bytes.is_empty() {
                    continue;
                }
                for (profile, score, sample_weights, flag) in evaluate(&bytes, &profiles).into_iter().take(3) {
                    if score < 0.4 {
                        continue;
                    }
                    best_for_port = best_for_port.max(score);
                    found.push(Candidate {
                        transport: t.clone(),
                        profile,
                        sample_weights,
                        stable_flag_seen: flag,
                        score,
                        raw_sample: escape(&bytes[..bytes.len().min(120)]),
                    });
                }
                if best_for_port >= 0.9 {
                    break 'tries;
                }
            }
        }
    }
    found.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
    found
}

pub fn detect_tcp(host: &str, port: u16, extra: &[Profile], dur: Duration) -> anyhow::Result<Vec<Candidate>> {
    let t = Transport::Tcp { host: host.into(), port };
    let bytes = capture(&t, dur, None)?;
    let mut profiles: Vec<Profile> = extra.to_vec();
    profiles.extend(builtin_profiles());
    Ok(evaluate(&bytes, &profiles)
        .into_iter()
        .take(4)
        .map(|(profile, score, sample_weights, flag)| Candidate {
            transport: t.clone(),
            profile,
            sample_weights,
            stable_flag_seen: flag,
            score,
            raw_sample: escape(&bytes[..bytes.len().min(120)]),
        })
        .collect())
}

/// Learn mode: given raw bytes captured while the indicator shows a steady
/// `displayed_kg` (> 0), build a profile that extracts exactly that number.
pub fn learn(bytes: &[u8], displayed_kg: f64) -> Option<Profile> {
    if displayed_kg <= 0.0 {
        return None;
    }
    let token = Regex::new(r"\d+(?:\.\d+)?").unwrap();
    let factors = [1.0, 10.0, 100.0, 1000.0, 0.1, 0.01, 0.001];
    let sep_sets: [Option<&str>; 2] = [None, Some(r"=\r\n\x02\x03")];
    let tol = (displayed_kg * 0.002).max(1.0);

    for seps in sep_sets {
        let probe = Profile {
            id: "probe".into(),
            name: String::new(),
            description: String::new(),
            format: Format::Regex,
            pattern: Some(".".into()),
            stable_pattern: None,
            unstable_pattern: None,
            reverse_digits: false,
            multiplier: 1.0,
            separators: seps.map(String::from),
            poll_command: None,
            poll_interval_ms: 250,
        };
        let c = probe.compile().ok()?;
        let (frames, _) = c.split(bytes);
        let texts: Vec<String> = frames.iter().filter(|f| f.len() >= 2).map(|f| String::from_utf8_lossy(f).to_string()).collect();
        if texts.len() < 3 {
            continue;
        }
        // Learn mode idea: the operator tells us the number on the display.
        // We look for that number among the numbers inside each frame,
        // trying: which number in the frame (index), digits reversed or not,
        // and a power-of-ten factor (e.g. the frame says 12.34 and the display
        // 12,340 kg). The combination that matches most frames becomes a new
        // regex profile, which is then verified against the same data.
        // (token index, reversed, factor) -> matching frames
        let mut best: Option<((usize, bool, f64), usize)> = None;
        for idx in 0..8 {
            for rev in [false, true] {
                for f in factors {
                    let hits = texts
                        .iter()
                        .filter(|t| {
                            token.find_iter(t).nth(idx).is_some_and(|m| {
                                let s: String = if rev { m.as_str().chars().rev().collect() } else { m.as_str().into() };
                                s.parse::<f64>().is_ok_and(|v| (v * f - displayed_kg).abs() <= tol)
                            })
                        })
                        .count();
                    if hits * 2 >= texts.len() && best.as_ref().is_none_or(|(_, h)| hits > *h) {
                        best = Some(((idx, rev, f), hits));
                    }
                }
            }
        }
        if let Some(((idx, rev, f), _)) = best {
            let skip = if idx == 0 { String::new() } else { format!(r"(?:\D*?\d+(?:\.\d+)?){{{idx}}}") };
            let pattern = format!(r"^{skip}\D*?(?P<sign>[+-])?(?P<weight>\d+(?:\.\d+)?)");
            let p = Profile {
                id: format!("learned-{}", chrono::Utc::now().format("%Y%m%d%H%M%S")),
                name: "Learned profile".into(),
                description: format!("Learned from display {displayed_kg} kg: number #{} in frame{}{}", idx + 1, if rev { ", reversed" } else { "" }, if f != 1.0 { format!(", x{f}") } else { String::new() }),
                format: Format::Regex,
                pattern: Some(pattern),
                stable_pattern: None,
                unstable_pattern: None,
                reverse_digits: rev,
                multiplier: f,
                separators: seps.map(String::from),
                poll_command: None,
                poll_interval_ms: 250,
            };
            // verify
            let cp = p.compile().ok()?;
            let ok = frames.iter().filter_map(|fr| cp.parse(fr)).filter(|r| (r.weight_kg - displayed_kg).abs() <= tol).count();
            if ok * 2 >= texts.len() {
                return Some(p);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluate_picks_toledo() {
        let mut bytes = Vec::new();
        for _ in 0..10 {
            bytes.extend_from_slice(b"\x02\x22\x20\x20012340000000\r\x55");
        }
        let r = evaluate(&bytes, &builtin_profiles());
        assert_eq!(r[0].0.id, "toledo");
        assert_eq!(r[0].2, vec![12340.0]);
    }

    #[test]
    fn evaluate_picks_st_gs_over_generic() {
        let bytes = b"ST,GS,+0012340kg\r\n".repeat(10);
        let r = evaluate(&bytes, &builtin_profiles());
        assert_eq!(r[0].0.id, "st-gs");
    }

    #[test]
    fn learn_odd_format() {
        // Unknown indicator: "S01 W 0012340 T 0000000 #A7"
        let bytes = b"S01 W 0012340 T 0000000 #47\r\n".repeat(8);
        let p = learn(&bytes, 12340.0).expect("learned");
        let c = p.compile().unwrap();
        assert_eq!(c.parse(b"S01 W 0015000 T 0000000 #47").unwrap().weight_kg, 15000.0);
    }

    #[test]
    fn learn_reversed_and_scaled() {
        let bytes = b"=0432100=0432100=0432100=0432100".to_vec();
        let p = learn(&bytes, 12340.0).expect("learned");
        assert!(p.reverse_digits);
        let bytes = b"WT:12.34\r\n".repeat(5);
        let p = learn(&bytes, 12340.0).expect("learned");
        assert_eq!(p.multiplier, 1000.0);
    }
}
