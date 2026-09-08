//! hwd — the device profile reader (D14: 机型是数据，不是代码).
//!
//! The single legal source for every machine-specific fact the platform
//! crates consume. No crate may hardcode a machine constant; they read it
//! here instead. The profile is data shipped inside the baked image at
//! `/etc/aginx/device.toml`; on dev hosts set `AGINX_DEVICE_TOML` to a
//! real `devices/<codename>/device.toml`.
//!
//! D14 law 2: there is NO default machine. A missing or malformed profile
//! is always a hard error — a fallback would be a hidden machine
//! assumption leaking back into the platform.
//!
//! Schema sections marked "registered" (`update`, `slots`, `paths.wlan`,
//! `paths.drm_card`) are parsed and carried but not yet consumed by any
//! platform code — they exist so the data has a home before the code
//! catches up. Everything else is live wiring (see ARCH.md D14).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::Deserialize;

/// Where the baked image carries the profile (build-rootfs.sh injects it).
pub const DEVICE_TOML_PATH: &str = "/etc/aginx/device.toml";
/// Dev-host override: point at a real `devices/<codename>/device.toml`.
pub const DEVICE_TOML_ENV: &str = "AGINX_DEVICE_TOML";

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Device {
    pub device: DeviceSection,
    pub panel: Panel,
    pub input: Input,
    pub audio: Audio,
    pub quirks: Quirks,
    pub affinity: Affinity,
    pub camera: Camera,
    pub paths: Paths,
    pub adb: Adb,
    /// [v1] registered only — dual-frozen with the first-gen trampoline.
    pub update: Option<UpdateSection>,
    /// [v1] registered only — the slot method name, not the mechanism.
    pub slots: Option<Slots>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceSection {
    /// Must equal the `devices/<codename>/` directory name.
    pub name: String,
    pub model: String,
    /// Grouping reference; drives nothing.
    pub soc: String,
    pub kernel: String,
    /// e.g. "vendor-boot" (redfin) vs a dtbo-style chain (enchilada).
    pub boot_style: String,
}

/// Panel geometry for non-DRM consumers only. Runtime display size is
/// DRM-enumerated truth; `width`/`height` here feed the voice HTML pins,
/// cam `--aspect` and host tests. `touch_max_*` is the native touch
/// sensor range the TouchReader scales from.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Panel {
    pub width: u32,
    pub height: u32,
    pub touch_max_x: u32,
    pub touch_max_y: u32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    pub ptt: InputPtt,
    pub term: InputTerm,
}

/// voice/ptt.rs — volume-down + power (push-to-talk) and volume-up (eye).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputPtt {
    pub device: String,
    pub volume_up_device: String,
    pub key_volume_down: u16,
    pub key_volume_up: u16,
    pub key_power: u16,
}

/// term/main.rs — touch + power event nodes.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputTerm {
    pub touch_device: String,
    pub power_device: String,
}

/// voice/audio.rs — bare-ioctl PCM pair. `channels = 1` is the capture
/// mono law (M42a); `playback_dup_mono` in `[quirks]` decides whether
/// speak() must copy L=R.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Audio {
    pub capture_pcm: String,
    pub playback_pcm: String,
    pub rate: u32,
    pub channels: u32,
    pub playback_channels: u32,
    pub volume_min: u32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Quirks {
    /// QUIN_TDM needs L=R copied into both channels; a codec with a real
    /// stereo path (WCD9340) sets this false.
    pub playback_dup_mono: bool,
    /// Full argv tail for the resident eye viewfinder (cam-shot).
    pub eye_stream_args: Vec<String>,
    /// Cam-shot argv tail for the QR blind-shot path.
    pub qr_scan_args: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Affinity {
    pub cam_cores: Vec<u32>,
    pub ui_cores: Vec<u32>,
    pub big_cores: Vec<u32>,
}

/// Sensor timing/registers/calibration live in `devices/<codename>/cam/`
/// C sources; only the numbers other crates need are here.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Camera {
    pub rear_sensor: String,
    pub dark_gain: u32,
    pub dark_dgain: u32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Paths {
    pub power_supply: String,
    /// [v1] registered.
    pub wlan: Option<String>,
    /// [v1] registered.
    pub drm_card: Option<String>,
}

/// [v1] registered only. The SWAP/BAK/STATE offsets are dual-frozen with
/// the first-gen trampoline; single-sided parameterization would break
/// the update flow's rollback story (ARCH.md D14 exemption).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateSection {
    pub layout: UpdateLayout,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateLayout {
    pub swap_offset_gib: u64,
    pub backup_offset_gib: u64,
    pub state_offset_gib: u64,
}

/// [v1] registered only — the slot method name (e.g. "per-lun-gpt").
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Slots {
    pub method: String,
    pub luns: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Adb {
    pub serial: String,
    pub fastboot_serial: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    #[error("hwd: missing {0} — image baked without a device profile")]
    Missing(PathBuf),
    #[error("hwd: cannot read {0}")]
    Io(PathBuf, #[source] std::io::Error),
    #[error("hwd: bad device profile {0}")]
    Parse(PathBuf, #[source] toml::de::Error),
}

/// The path the profile is read from: `AGINX_DEVICE_TOML` wins (dev
/// hosts), else the baked-in `/etc/aginx/device.toml`.
pub fn default_path() -> PathBuf {
    std::env::var_os(DEVICE_TOML_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEVICE_TOML_PATH))
}

pub fn from_path(path: &Path) -> Result<Device, ProfileError> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            ProfileError::Missing(path.to_path_buf())
        } else {
            ProfileError::Io(path.to_path_buf(), e)
        }
    })?;
    toml::from_str(&text).map_err(|e| ProfileError::Parse(path.to_path_buf(), e))
}

static PROFILE: OnceLock<Device> = OnceLock::new();

/// Process-wide profile. Hard-exits (fixed D14 message) when absent or
/// malformed — there is no default machine to fall back to.
pub fn load_or_exit() -> &'static Device {
    PROFILE.get_or_init(|| match from_path(&default_path()) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn redfin() -> Device {
        let p = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../devices/redfin/device.toml");
        from_path(&p).expect("real redfin profile must parse")
    }

    /// The schema and the real device data stay in lockstep: this reads
    /// the actual committed profile, not a fixture copy.
    #[test]
    fn redfin_profile_is_schema_truth() {
        let d = redfin();
        assert_eq!(d.device.name, "redfin"); // D14-exempt: asserts real device data
        assert_eq!(d.device.boot_style, "vendor-boot"); // D14-exempt
        assert_eq!((d.panel.width, d.panel.height), (1080, 2340)); // D14-exempt
        assert_eq!(
            (d.panel.touch_max_x, d.panel.touch_max_y),
            (1080, 2340)
        ); // D14-exempt
        assert_eq!(d.input.ptt.device, "/dev/input/event1"); // D14-exempt
        assert_eq!(d.input.ptt.volume_up_device, "/dev/input/event0"); // D14-exempt
        assert_eq!(d.input.ptt.key_volume_down, 114); // D14-exempt
        assert_eq!(d.input.ptt.key_volume_up, 115); // D14-exempt
        assert_eq!(d.input.ptt.key_power, 116); // D14-exempt
        assert_eq!(d.input.term.touch_device, "/dev/input/event2"); // D14-exempt
        assert_eq!(d.input.term.power_device, "/dev/input/event1"); // D14-exempt
        assert_eq!(d.audio.rate, 48000); // D14-exempt
        assert_eq!(d.audio.channels, 1); // D14-exempt
        assert_eq!(d.audio.playback_channels, 2); // D14-exempt
        assert_eq!(d.audio.volume_min, 20); // D14-exempt
        assert!(d.quirks.playback_dup_mono); // D14-exempt
        assert!(d.quirks.eye_stream_args.contains(&"--rot".into())); // D14-exempt
        assert!(d.quirks.eye_stream_args.contains(&"90".into())); // D14-exempt
        assert_eq!(d.affinity.cam_cores, vec![6, 7]); // D14-exempt
        assert_eq!(d.affinity.ui_cores, vec![0, 1, 2, 3, 4, 5]); // D14-exempt
        assert_eq!(d.affinity.big_cores, vec![6, 7]); // D14-exempt
        assert_eq!(d.camera.rear_sensor, "imx481"); // D14-exempt
        assert_eq!((d.camera.dark_gain, d.camera.dark_dgain), (16, 2)); // D14-exempt
        assert_eq!(
            d.paths.power_supply,
            "/sys/class/power_supply/battery"
        ); // D14-exempt
        assert_eq!(d.adb.serial, "aginxosredfin"); // D14-exempt
    }

    /// [v1] registered sections parse and carry their data.
    #[test]
    fn registered_v1_sections_parse() {
        let d = redfin();
        let u = d.update.as_ref().expect("update registered"); // D14-exempt
        assert_eq!(u.layout.swap_offset_gib, 8); // D14-exempt
        assert_eq!(u.layout.backup_offset_gib, 32); // D14-exempt
        assert_eq!(u.layout.state_offset_gib, 64); // D14-exempt
        let s = d.slots.as_ref().expect("slots registered"); // D14-exempt
        assert_eq!(s.method, "per-lun-gpt"); // D14-exempt
        assert_eq!(s.luns.len(), 6); // D14-exempt
        assert_eq!(d.paths.wlan.as_deref(), Some("wlan0")); // D14-exempt
    }

    /// D14 law 2: a missing profile is the fixed fail-fast message.
    #[test]
    fn missing_profile_has_fixed_message() {
        let err = from_path(Path::new("/definitely/not/a/profile.toml"))
            .expect_err("must fail");
        assert!(
            err.to_string().contains("image baked without a device profile"),
            "got: {err}"
        );
    }

    /// D14 law 2: no hidden defaults — a profile missing a wired section
    /// must fail to parse, never silently fall back.
    #[test]
    fn no_hidden_defaults() {
        let minimal = "[device]\nname = \"x\"\n";
        let err = toml::from_str::<Device>(minimal).expect_err("must fail");
        assert!(err.to_string().contains("missing field"));
    }

    /// Typos in the data file surface as parse errors (deny_unknown_fields).
    #[test]
    fn unknown_fields_are_rejected() {
        let bad = "[device]\nname = \"x\"\nnmae = \"typo\"\n";
        assert!(toml::from_str::<Device>(bad).is_err());
    }
}
