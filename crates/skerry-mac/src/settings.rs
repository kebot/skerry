use crate::input::InputSettings;
use crate::launch_options::LaunchMemory;
use crate::video::Codec;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::BTreeMap, io, path::PathBuf};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Key {
    Fps,
    Jpeg,
    PaintOver,
    PaintQuality,
    RateControl,
    Bitrate,
    Crf,
}

impl Key {
    pub const ALL: [Self; 7] = [
        Self::Fps,
        Self::Jpeg,
        Self::PaintOver,
        Self::PaintQuality,
        Self::RateControl,
        Self::Bitrate,
        Self::Crf,
    ];

    pub fn wire(self) -> &'static str {
        match self {
            Self::Fps => "framerate",
            Self::Jpeg => "jpeg_quality",
            Self::PaintOver => "use_paint_over_quality",
            Self::PaintQuality => "paint_over_jpeg_quality",
            Self::RateControl => "rate_control_mode",
            Self::Bitrate => "video_bitrate",
            Self::Crf => "video_crf",
        }
    }

    fn read(self, value: &Value) -> Option<u32> {
        if self == Self::RateControl {
            return match value.as_str()? {
                "cbr" => Some(0),
                "crf" => Some(1),
                _ => None,
            };
        }
        if self == Self::PaintOver {
            value.as_bool().map(u32::from)
        } else {
            value
                .as_f64()
                .filter(|v| (0.0..=f64::from(u32::MAX)).contains(v))
                .map(|v| v.round() as u32)
        }
    }

    pub fn encode(self, value: u32) -> Value {
        if self == Self::RateControl {
            return json!(if value == 0 { "cbr" } else { "crf" });
        }
        if self == Self::PaintOver {
            json!(value != 0)
        } else {
            json!(value)
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Control {
    pub current: u32,
    pub initial: u32,
    pub min: u32,
    pub max: u32,
    pub locked: bool,
}

#[derive(Clone)]
pub struct Settings {
    controls: BTreeMap<Key, Control>,
    pub codecs: Vec<Codec>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            controls: BTreeMap::new(),
            codecs: Codec::ALL
                .into_iter()
                .filter(|codec| codec.supported())
                .collect(),
        }
    }
}

impl Settings {
    pub fn announce(&mut self, payload: &Value) {
        self.codecs = Codec::ALL
            .into_iter()
            .filter(|codec| codec.supported() && encoder_allowed(&payload["encoder"], codec.wire()))
            .collect();
        for key in Key::ALL {
            if key == Key::RateControl {
                let mode = &payload[key.wire()];
                let control = key.read(&mode["value"]).map(|current| {
                    let locked = payload["enable_rate_control"]["value"] == false
                        || mode["locked"] == true
                        || !["cbr", "crf"]
                            .iter()
                            .all(|name| encoder_allowed(mode, name));
                    Control {
                        current,
                        initial: self.get(key).map_or(current, |old| old.initial),
                        min: 0,
                        max: 1,
                        locked,
                    }
                });
                self.controls.remove(&key);
                if let Some(control) = control {
                    self.controls.insert(key, control);
                }
                continue;
            }
            if let Some(control) = parse(key, &payload[key.wire()], self.get(key)) {
                self.controls.insert(key, control);
            } else {
                self.controls.remove(&key);
            }
        }
    }

    pub fn live(&mut self, payload: &Value) {
        for (&key, control) in &mut self.controls {
            if let Some(value) = key.read(&payload[key.wire()]) {
                control.current = value.clamp(control.min, control.max);
            }
        }
    }

    pub fn get(&self, key: Key) -> Option<Control> {
        self.controls.get(&key).copied()
    }

    pub fn request(&self, key: Key, value: u32) -> Option<u32> {
        self.get(key)
            .filter(|control| !control.locked && control.min != control.max)
            .map(|control| value.clamp(control.min, control.max))
    }
}

pub fn encoder_allowed(encoder: &Value, name: &str) -> bool {
    if encoder["locked"] == true {
        return encoder["value"] == name;
    }
    encoder["allowed"]
        .as_array()
        .is_none_or(|allowed| allowed.iter().any(|value| value == name))
}

fn parse(key: Key, entry: &Value, previous: Option<Control>) -> Option<Control> {
    let announced = if key == Key::PaintOver {
        key.read(&entry["value"])
    } else {
        key.read(&entry["default"])
            .or_else(|| key.read(&entry["value"]))
    }?;
    let range = if key == Key::PaintOver {
        (0, 1)
    } else {
        (
            key.read(&entry["min"]).unwrap_or(announced),
            key.read(&entry["max"]).unwrap_or(announced),
        )
    };
    let (min, max) = range;
    if min > max {
        return None;
    }
    let current = announced.clamp(min, max);
    let locked = entry["locked"].as_bool() == Some(true);
    let initial = previous
        .filter(|control| !locked && !control.locked)
        .map_or(current, |control| control.initial.clamp(min, max));
    Some(Control {
        current,
        initial,
        min,
        max,
        locked,
    })
}

#[derive(Clone, Default)]
pub struct ScreenPolicy {
    pub alignment: Option<(bool, bool)>,
    pub dpi: Option<u16>,
    pub dpi_choices: Vec<u16>,
}

#[derive(Clone, Copy)]
pub enum ScreenChange {
    Alignment(bool),
    Dpi(u16),
}

impl ScreenPolicy {
    pub fn from_server(payload: &Value) -> Self {
        let aligned = &payload["force_aligned_resolution"];
        let dpi = &payload["scaling_dpi"];
        let number = |value: &Value| {
            value
                .as_u64()
                .and_then(|n| u16::try_from(n).ok())
                .or_else(|| value.as_str()?.parse().ok())
        };
        Self {
            alignment: aligned["value"]
                .as_bool()
                .map(|value| (value, aligned["locked"].as_bool() == Some(true))),
            dpi: number(&dpi["value"]),
            dpi_choices: if dpi["locked"] == true || dpi["overridden"] == true {
                Vec::new()
            } else {
                dpi["allowed"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(number)
                    .collect()
            },
        }
    }

    pub fn display_dpi(&self, scale: f32) -> Option<u16> {
        let target = (96. * scale).round() as u16;
        self.dpi_choices
            .iter()
            .copied()
            .min_by_key(|&dpi| (dpi.abs_diff(target), dpi))
    }

    pub fn apply(&mut self, change: ScreenChange) -> Option<Value> {
        match change {
            ScreenChange::Alignment(value) if self.alignment.is_some_and(|(_, locked)| !locked) => {
                self.alignment = Some((value, false));
                Some(json!({"force_aligned_resolution": value}))
            }
            ScreenChange::Dpi(value)
                if self.dpi != Some(value) && self.dpi_choices.contains(&value) =>
            {
                self.dpi = Some(value);
                Some(json!({"scaling_dpi": value}))
            }
            _ => None,
        }
    }
}

pub type VideoSettings = BTreeMap<Key, u32>;

#[derive(Default, Deserialize, Serialize)]
pub struct Preferences {
    #[serde(default)]
    pub codec: Codec,
    #[serde(default, skip_serializing_if = "InputSettings::is_default")]
    pub input: InputSettings,
    /// The Launch Dialog options each app last launched with, by app id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub launch: BTreeMap<String, LaunchMemory>,
    #[serde(flatten)]
    pub video: VideoSettings,
}

fn path(server: &str) -> Option<PathBuf> {
    Some(
        PathBuf::from(std::env::var_os("HOME")?)
            .join("Library/Application Support/SealSkin Native")
            .join(format!("{server}.json")),
    )
}

pub fn load(server: &str) -> Preferences {
    path(server)
        .and_then(|path| std::fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub fn save(server: &str, preferences: &Preferences) -> io::Result<()> {
    let path = path(server).ok_or_else(|| io::Error::other("Home directory unavailable"))?;
    let dir = path.parent().expect("preferences directory");
    std::fs::create_dir_all(dir)?;
    let mut file = tempfile::NamedTempFile::new_in(dir)?;
    serde_json::to_writer(&mut file, preferences)?;
    file.as_file().sync_all()?;
    file.persist(&path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reconciles_saved_values_with_server_policy_and_live_updates() {
        let mut settings = Settings::default();
        settings.announce(&json!({
            "framerate": {"value": [15, 60], "default": 24, "min": 15, "max": 60},
            "jpeg_quality": {"value": [50, 50], "default": 50, "min": 50, "max": 50},
            "use_paint_over_quality": {"value": false, "locked": true}
        }));
        assert_eq!(settings.request(Key::Fps, 120), Some(60));
        assert_eq!(settings.request(Key::Jpeg, 90), None);
        assert_eq!(settings.request(Key::PaintOver, 1), None);
        assert_eq!(settings.request(Key::PaintQuality, 90), None);
        assert_eq!(settings.get(Key::Fps).unwrap().current, 24);
        settings.live(&json!({"framerate": 45}));
        assert_eq!(settings.get(Key::Fps).unwrap().current, 45);
        assert_eq!(settings.get(Key::Fps).unwrap().initial, 24);
    }

    #[test]
    fn automatic_dpi_matches_the_closest_server_scale() {
        let settings =
            json!({"scaling_dpi": {"value": "96", "allowed": ["96", "120", "144", "168", "192"]}});
        let policy = ScreenPolicy::from_server(&settings);
        assert_eq!(policy.display_dpi(2.), Some(192));
        assert_eq!(policy.display_dpi(1.), Some(96));
        assert_eq!(policy.display_dpi(1.6), Some(144));
        let mut locked = settings;
        locked["scaling_dpi"]["overridden"] = json!(true);
        assert_eq!(ScreenPolicy::from_server(&locked).display_dpi(2.), None);
    }

    #[test]
    fn legacy_video_preferences_load_and_codec_round_trips() {
        let mut prefs: Preferences =
            serde_json::from_value(json!({"jpeg": 45, "fps": 25})).unwrap();
        assert_eq!(prefs.codec, Codec::H264);
        assert_eq!(prefs.video.get(&Key::Jpeg), Some(&45));
        prefs.codec = Codec::Hevc;
        prefs.video.insert(Key::Bitrate, 80_000);
        let restored: Preferences =
            serde_json::from_value(serde_json::to_value(prefs).unwrap()).unwrap();
        assert_eq!(restored.codec, Codec::Hevc);
        assert_eq!(restored.video.get(&Key::Bitrate), Some(&80_000));
    }

    #[test]
    fn input_switches_are_persisted_alongside_existing_video_preferences() {
        let saved = json!({"jpeg": 45, "input": {"command_as_control": true, "mac_ime": true}});
        let mut prefs: Preferences = serde_json::from_value(saved).unwrap();
        assert_eq!(prefs.video.get(&Key::Jpeg), Some(&45));
        assert!(prefs.input.command_as_control && prefs.input.mac_ime);
        assert!(!prefs.input.remote_cursor && prefs.input.trackpad_scroll);
        crate::input::InputFeature::RemoteCursor.set(&mut prefs.input, true);
        let restored: Preferences =
            serde_json::from_value(serde_json::to_value(&prefs).unwrap()).unwrap();
        assert!(restored.input.remote_cursor && restored.input.mac_ime);
    }

    #[test]
    fn rate_settings_respect_server_bounds_and_locks() {
        let mut settings = Settings::default();
        settings.announce(&json!({
            "encoder": {"value": "jpeg", "locked": true, "allowed": ["jpeg", "h265enc"]},
            "rate_control_mode": {"value": "cbr", "allowed": ["cbr", "crf"]},
            "video_bitrate": {"default": 8000, "min": 100, "max": 1_000_000},
            "video_crf": {"default": 25, "min": 5, "max": 50, "locked": true}
        }));
        assert_eq!(settings.codecs, [Codec::Jpeg]);
        assert_eq!(settings.request(Key::Bitrate, 80_000), Some(80_000));
        assert_eq!(settings.request(Key::Crf, 40), None);
        assert_eq!(
            settings
                .request(Key::RateControl, 1)
                .map(|value| Key::RateControl.encode(value)),
            Some(json!("crf"))
        );
        settings.announce(&json!({"rate_control_mode": {"value": "cbr"}, "enable_rate_control": {"value": false}}));
        assert_eq!(settings.request(Key::RateControl, 1), None);
    }

    #[test]
    fn screen_policy_only_sends_editable_and_advertised_values() {
        let mut policy = ScreenPolicy::from_server(&json!({
            "force_aligned_resolution": {"value": false, "locked": true},
            "scaling_dpi": {"value": "96", "allowed": ["96", 192]}
        }));
        assert!(policy.apply(ScreenChange::Alignment(true)).is_none());
        assert!(policy.apply(ScreenChange::Dpi(144)).is_none());
        assert_eq!(
            policy.apply(ScreenChange::Dpi(192)),
            Some(json!({"scaling_dpi": 192}))
        );
        assert_eq!(policy.dpi, Some(192));
    }

    #[test]
    fn jpeg_is_unavailable_only_when_the_server_excludes_it() {
        let mut settings = Settings::default();
        assert!(settings.codecs.contains(&Codec::Jpeg));
        settings
            .announce(&json!({"encoder": {"value": "h264enc", "allowed": ["h264enc", "jpeg"]}}));
        assert!(settings.codecs.contains(&Codec::Jpeg));
        settings.announce(&json!({"encoder": {"value": "h264enc", "allowed": ["h264enc"]}}));
        assert!(!settings.codecs.contains(&Codec::Jpeg));
    }
}
