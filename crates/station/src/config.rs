//! Station settings, stored as JSON in the data directory. Scale settings can
//! also be pushed from the dashboard (Stations), which replaces the old
//! per-client copies of the program.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use wb_scale::{find_profile, Profile, ScaleConfig, StabilityConfig, Transport};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StationScaleConfig {
    pub transport: Transport,
    /// A built-in profile id (toledo, st-gs, yaohua-reversed, weight-unit, sign6, generic-number)...
    #[serde(default)]
    pub profile_id: Option<String>,
    /// ...or a full custom/learned profile.
    #[serde(default)]
    pub profile: Option<Profile>,
    #[serde(default)]
    pub stability: StabilityConfig,
    /// Allow supervisors to type a weight when the scale is down (flagged on the ticket).
    #[serde(default)]
    pub allow_manual: bool,
    #[serde(default = "d_cap")]
    pub max_capacity_kg: f64,
    /// Readings below this count as an empty deck.
    #[serde(default = "d_min")]
    pub min_weight_kg: f64,
}

fn d_cap() -> f64 {
    100_000.0
}
fn d_min() -> f64 {
    50.0
}

impl StationScaleConfig {
    pub fn engine_config(&self) -> anyhow::Result<ScaleConfig> {
        let profile = match (&self.profile, &self.profile_id) {
            (Some(p), _) => p.clone(),
            (None, Some(id)) => find_profile(id, &[]).ok_or_else(|| anyhow::anyhow!("unknown profile '{id}'"))?,
            (None, None) => find_profile("generic-number", &[]).unwrap(),
        };
        Ok(ScaleConfig { transport: self.transport.clone(), profile, stability: self.stability.clone() })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PrinterConfig {
    /// Print from the browser instead (any printer, via the OS dialog)
    #[default]
    Browser,
    /// Network receipt printer (raw port, usually 9100)
    Tcp { host: String, #[serde(default = "d9100")] port: u16 },
    /// Serial/USB-serial receipt printer
    Serial { port: String, #[serde(default = "d9600")] baud: u32 },
    /// Write to a file or device path (e.g. /dev/usb/lp0, \\\\.\\USB001 share)
    File { path: String },
    /// Pipe the ticket text to a command (e.g. `lp -d TM-T20`)
    Command { command: String },
}

fn d9100() -> u16 {
    9100
}
fn d9600() -> u32 {
    9600
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct StationConfig {
    #[serde(default)]
    pub server_url: String,
    #[serde(default)]
    pub station_key: String,
    #[serde(default)]
    pub scale: Option<StationScaleConfig>,
    #[serde(default)]
    pub printer: PrinterConfig,
    /// Also print a copy automatically when the second weight is taken
    #[serde(default)]
    pub auto_print: bool,
    /// The last scale config received from the server, to notice changes
    #[serde(default)]
    pub server_scale_applied: Option<serde_json::Value>,
}

impl StationConfig {
    pub fn path(dir: &Path) -> PathBuf {
        dir.join("station.json")
    }
    pub fn load(dir: &Path) -> StationConfig {
        std::fs::read_to_string(Self::path(dir)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }
    pub fn save(&self, dir: &Path) -> anyhow::Result<()> {
        let tmp = dir.join("station.json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(tmp, Self::path(dir))?;
        Ok(())
    }
    pub fn is_enrolled(&self) -> bool {
        !self.server_url.is_empty() && !self.station_key.is_empty()
    }
}
