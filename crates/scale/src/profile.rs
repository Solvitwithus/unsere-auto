//! Indicator protocol profiles.
//!
//! Almost every weighbridge indicator streams its displayed weight as a short
//! ASCII frame, many times a second. A profile describes how to cut the byte
//! stream into frames and how to pull the weight (and, if present, the
//! stable/motion flag) out of each frame. Profiles are plain data, so new
//! indicators are supported by adding a JSON file, not by recompiling.

use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    /// Text frame matched by `pattern` (named groups: weight, sign, unit, status).
    #[default]
    Regex,
    /// Mettler Toledo continuous output: STX SWA SWB SWC wwwwww tttttt CR [CHK].
    /// Supported by Toledo and by most Keli / Yaohua / generic indicators.
    Toledo,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Profile {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub format: Format,
    /// Regex with a `weight` named group (required for `Format::Regex`).
    #[serde(default)]
    pub pattern: Option<String>,
    /// If set and the frame matches, the indicator reports the weight stable.
    #[serde(default)]
    pub stable_pattern: Option<String>,
    /// If set and the frame matches, the indicator reports motion.
    #[serde(default)]
    pub unstable_pattern: Option<String>,
    /// Some indicators (e.g. Yaohua XK3190 "tF=0") send the digits backwards.
    #[serde(default)]
    pub reverse_digits: bool,
    /// Multiply the parsed number by this to get kilograms (after unit handling).
    #[serde(default = "one")]
    pub multiplier: f64,
    /// Bytes that end/separate frames. Defaults to CR, LF, STX, ETX.
    #[serde(default)]
    pub separators: Option<String>,
    /// For demand-mode indicators: command sent every `poll_interval_ms`.
    /// Escapes \r \n \xNN are supported.
    #[serde(default)]
    pub poll_command: Option<String>,
    #[serde(default = "default_poll")]
    pub poll_interval_ms: u64,
}

fn one() -> f64 {
    1.0
}
fn default_poll() -> u64 {
    250
}

pub const DEFAULT_SEPARATORS: &[u8] = b"\r\n\x02\x03";

impl Profile {
    pub fn separator_bytes(&self) -> Vec<u8> {
        match &self.separators {
            Some(s) => unescape(s),
            None => DEFAULT_SEPARATORS.to_vec(),
        }
    }

    pub fn poll_bytes(&self) -> Option<Vec<u8>> {
        self.poll_command.as_deref().map(unescape)
    }

    pub fn compile(&self) -> anyhow::Result<CompiledProfile> {
        let build = |p: &str| -> anyhow::Result<Regex> {
            Ok(RegexBuilder::new(p).case_insensitive(true).build()?)
        };
        let pattern = match (&self.format, &self.pattern) {
            (Format::Regex, Some(p)) => Some(build(p)?),
            (Format::Regex, None) => anyhow::bail!("profile {} needs a pattern", self.id),
            _ => None,
        };
        Ok(CompiledProfile {
            profile: self.clone(),
            pattern,
            stable: self.stable_pattern.as_deref().map(build).transpose()?,
            unstable: self.unstable_pattern.as_deref().map(build).transpose()?,
            separators: self.separator_bytes(),
        })
    }
}

/// Result of parsing one frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Parsed {
    pub weight_kg: f64,
    /// `Some` when the indicator itself reports stable/motion.
    pub stable: Option<bool>,
}

#[derive(Clone, Debug)]
pub struct CompiledProfile {
    pub profile: Profile,
    pattern: Option<Regex>,
    stable: Option<Regex>,
    unstable: Option<Regex>,
    pub separators: Vec<u8>,
}

impl CompiledProfile {
    /// Split a byte buffer into frames. Returns (frames, unconsumed tail).
    pub fn split<'a>(&self, buf: &'a [u8]) -> (Vec<&'a [u8]>, &'a [u8]) {
        let mut frames = Vec::new();
        let mut start = 0;
        for (i, b) in buf.iter().enumerate() {
            if self.separators.contains(b) {
                if i > start {
                    frames.push(&buf[start..i]);
                }
                start = i + 1;
            }
        }
        (frames, &buf[start..])
    }

    pub fn parse(&self, frame: &[u8]) -> Option<Parsed> {
        match self.profile.format {
            Format::Toledo => parse_toledo(frame, self.profile.multiplier),
            Format::Regex => self.parse_regex(frame),
        }
    }

    fn parse_regex(&self, frame: &[u8]) -> Option<Parsed> {
        let text = String::from_utf8_lossy(frame);
        let caps = self.pattern.as_ref()?.captures(&text)?;
        let mut digits: String = caps.name("weight")?.as_str().chars().filter(|c| !c.is_whitespace()).collect();
        if self.profile.reverse_digits {
            digits = digits.chars().rev().collect();
        }
        let mut value: f64 = digits.parse().ok()?;
        if caps.name("sign").map(|s| s.as_str() == "-").unwrap_or(false) {
            value = -value;
        }
        if let Some(unit) = caps.name("unit") {
            value *= match unit.as_str().to_ascii_lowercase().as_str() {
                "t" => 1000.0,
                "lb" => 0.453_592_37,
                "g" => 0.001,
                _ => 1.0,
            };
        }
        value *= self.profile.multiplier;
        if !value.is_finite() {
            return None;
        }
        let stable = if self.unstable.as_ref().is_some_and(|r| r.is_match(&text)) {
            Some(false)
        } else if self.stable.as_ref().is_some_and(|r| r.is_match(&text)) {
            Some(true)
        } else if self.stable.is_some() {
            // A stable marker exists for this protocol but is absent.
            Some(false)
        } else {
            None
        };
        Some(Parsed { weight_kg: round3(value), stable })
    }
}

/// Mettler Toledo continuous output (frame without the leading STX / trailing CR).
fn parse_toledo(frame: &[u8], multiplier: f64) -> Option<Parsed> {
    if frame.len() < 15 {
        return None;
    }
    let (swa, swb) = (frame[0], frame[1]);
    // Status bytes always have bit 5 set and are printable.
    if frame[..3].iter().any(|b| b & 0x20 == 0 || *b > 0x7f) {
        return None;
    }
    let w = &frame[3..9];
    if !w.iter().all(|b| b.is_ascii_digit() || *b == b' ') || !frame[9..15].iter().all(|b| b.is_ascii_digit() || *b == b' ') {
        return None;
    }
    let raw: f64 = std::str::from_utf8(w).ok()?.trim().replace(' ', "0").parse().ok()?;
    let factor = match swa & 0x07 {
        0 => 100.0,
        1 => 10.0,
        2 => 1.0,
        3 => 0.1,
        4 => 0.01,
        5 => 0.001,
        6 => 0.0001,
        _ => 0.00001,
    };
    let mut value = raw * factor;
    if swb & 0x02 != 0 {
        value = -value;
    }
    if swb & 0x04 != 0 {
        return None; // over/under range
    }
    let motion = swb & 0x08 != 0;
    Some(Parsed { weight_kg: round3(value * multiplier), stable: Some(!motion) })
}

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

/// Decode \r \n \t \\ and \xNN escapes.
pub fn unescape(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 1 < b.len() {
            match b[i + 1] {
                b'r' => { out.push(b'\r'); i += 2; continue; }
                b'n' => { out.push(b'\n'); i += 2; continue; }
                b't' => { out.push(b'\t'); i += 2; continue; }
                b'\\' => { out.push(b'\\'); i += 2; continue; }
                b'x' if i + 3 < b.len() => {
                    if let Ok(v) = u8::from_str_radix(std::str::from_utf8(&b[i + 2..i + 4]).unwrap_or(""), 16) {
                        out.push(v);
                        i += 4;
                        continue;
                    }
                }
                _ => {}
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

/// Render raw bytes printable for logs/UI (non-printables as <XX>).
pub fn escape(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len());
    for &b in bytes {
        match b {
            0x20..=0x7e => s.push(b as char),
            b'\r' => s.push_str("<CR>"),
            b'\n' => s.push_str("<LF>"),
            0x02 => s.push_str("<STX>"),
            0x03 => s.push_str("<ETX>"),
            _ => s.push_str(&format!("<{b:02X}>")),
        }
    }
    s
}

fn rx(id: &str, name: &str, desc: &str, pattern: &str) -> Profile {
    Profile {
        id: id.into(),
        name: name.into(),
        description: desc.into(),
        format: Format::Regex,
        pattern: Some(pattern.into()),
        stable_pattern: None,
        unstable_pattern: None,
        reverse_digits: false,
        multiplier: 1.0,
        separators: None,
        poll_command: None,
        poll_interval_ms: 250,
    }
}

/// Built-in profiles, most specific first. Auto-detect tries them in this order.
pub fn builtin_profiles() -> Vec<Profile> {
    let mut v = Vec::new();

    v.push(Profile {
        id: "toledo".into(),
        name: "Toledo continuous".into(),
        description: "Mettler Toledo continuous output. Also offered by most Keli, Yaohua and Chinese indicators (often called 'tF=1' or 'Toledo').".into(),
        format: Format::Toledo,
        pattern: None,
        ..rx("", "", "", "")
    });

    let mut st = rx(
        "st-gs",
        "ST,GS / US,GS (A&D style)",
        "Frames like 'ST,GS,+0012345kg'. Used by A&D, CAS, many OEM indicators. ST = stable, US = motion.",
        r"(?P<status>ST|US|OL)\s*,\s*(?:GS|NT|TR|G|N)?\s*,?\s*(?P<sign>[+-])?\s*(?P<weight>\d+(?:\.\d+)?)\s*(?P<unit>kg|lb|t|g)?",
    );
    st.stable_pattern = Some(r"^\W*ST".into());
    st.unstable_pattern = Some(r"^\W*(US|OL)".into());
    v.push(st);

    let mut yh = rx(
        "yaohua-reversed",
        "Yaohua XK3190 (reversed, '=')",
        "Yaohua XK3190 continuous mode tF=0: '=' then the digits in reverse order.",
        r"^\s*(?P<sign>-)?(?P<weight>[\d.]{5,9})\s*$",
    );
    yh.reverse_digits = true;
    yh.separators = Some(r"=\r\n\x02\x03".into());
    v.push(yh);

    let mut wu = rx(
        "weight-unit",
        "Weight + unit (Rinstrum / Avery / Cardinal style)",
        "Frames like '  12340 kg G' or 'G  12.34 t'. A standalone M or MOTION marks movement.",
        r"(?P<sign>[+-])?\s*(?P<weight>\d+(?:\.\d+)?)\s*(?P<unit>kg|lb|t)\b",
    );
    wu.unstable_pattern = Some(r"(\bM\b|MOTION|\bUS\b)".into());
    v.push(wu);

    v.push(rx(
        "sign6",
        "Sign + 6 digits",
        "Frames like '+000140' optionally followed by extra digits (common on budget indicators).",
        r"(?P<sign>[+-])\s?(?P<weight>\d{6}(?:\.\d+)?)",
    ));

    v.push(rx(
        "generic-number",
        "Generic: first number in frame",
        "Last resort: takes the first number in each frame. Use Learn mode for anything odd.",
        r"(?P<sign>[+-])?\s*(?P<weight>\d+(?:\.\d+)?)",
    ));
    v
}

pub fn find_profile(id: &str, extra: &[Profile]) -> Option<Profile> {
    extra.iter().cloned().chain(builtin_profiles()).find(|p| p.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(id: &str) -> CompiledProfile {
        find_profile(id, &[]).unwrap().compile().unwrap()
    }

    #[test]
    fn st_gs() {
        let c = p("st-gs");
        assert_eq!(c.parse(b"ST,GS,+0012340kg"), Some(Parsed { weight_kg: 12340.0, stable: Some(true) }));
        assert_eq!(c.parse(b"US,GS,+0012340kg").unwrap().stable, Some(false));
        assert_eq!(c.parse(b"ST,NT,-00012.5t").unwrap().weight_kg, -12500.0);
    }

    #[test]
    fn toledo() {
        // SWA=0x22 (decimal code 2 -> x1), SWB=0x20 (gross, positive, stable), SWC=0x20
        let c = p("toledo");
        let f = b"\x22\x20\x20012340000000";
        assert_eq!(c.parse(f), Some(Parsed { weight_kg: 12340.0, stable: Some(true) }));
        let moving = b"\x22\x28\x20012340000000";
        assert_eq!(c.parse(moving).unwrap().stable, Some(false));
        let neg = b"\x22\x22\x20000120000000";
        assert_eq!(c.parse(neg).unwrap().weight_kg, -120.0);
        // decimal code 1 -> x10 (20 kg increments shown as 1234 -> 12340)
        assert_eq!(c.parse(b"\x21\x20\x20001234000000").unwrap().weight_kg, 12340.0);
    }

    #[test]
    fn yaohua_reversed() {
        let c = p("yaohua-reversed");
        let (frames, _) = c.split(b"=0432100=0432100=");
        assert_eq!(frames.len(), 2);
        assert_eq!(c.parse(frames[0]).unwrap().weight_kg, 12340.0);
    }

    #[test]
    fn farisi_sign6() {
        let c = p("sign6");
        assert_eq!(c.parse(b"+00014000").unwrap().weight_kg, 140.0);
        assert_eq!(c.parse(b"+012340").unwrap().weight_kg, 12340.0);
    }

    #[test]
    fn weight_unit() {
        let c = p("weight-unit");
        assert_eq!(c.parse(b"   12340 kg G"), Some(Parsed { weight_kg: 12340.0, stable: None }));
        assert_eq!(c.parse(b"   12340 kg G M").unwrap().stable, Some(false));
        assert_eq!(c.parse(b"G 12.34 t").unwrap().weight_kg, 12340.0);
    }

    #[test]
    fn split_keeps_tail() {
        let c = p("generic-number");
        let (f, tail) = c.split(b"123\r\n456\r\n78");
        assert_eq!(f, vec![&b"123"[..], &b"456"[..]]);
        assert_eq!(tail, b"78");
    }

    #[test]
    fn unescape_works() {
        assert_eq!(unescape(r"P\r\n"), b"P\r\n");
        assert_eq!(unescape(r"\x05Q"), b"\x05Q");
    }
}
