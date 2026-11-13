//! wb-scale: talk to (almost) any weighbridge indicator.
//!
//! * [`transport`]: RS-232/RS-485/USB-serial and TCP/IP links
//! * [`profile`]: data-driven protocol profiles (Toledo, ST/GS, Yaohua, ...)
//! * [`stability`]: motion detection
//! * [`engine`]: background reader publishing live [`engine::ScaleState`]
//! * [`detect`]: auto-detect port/baud/protocol, and Learn mode

pub mod detect;
pub mod engine;
pub mod profile;
pub mod stability;
pub mod transport;

pub use engine::{start, Reading, ScaleConfig, ScaleHandle, ScaleState};
pub use profile::{builtin_profiles, find_profile, Profile};
pub use stability::StabilityConfig;
pub use transport::Transport;
