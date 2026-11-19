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
}

impl Key {
    pub const ALL: [Self; 4] = [Self::Fps, Self::Jpeg, Self::PaintOver, Self::PaintQuality];

    pub fn wire(self) -> &'static str {
        match self {
            Self::Fps => "framerate",
            Self::Jpeg => "jpeg_quality",
            Self::PaintOver => "use_paint_over_quality",
            Self::PaintQuality => "paint_over_jpeg_quality",
        }
    }

    fn read(self, value: &Value) -> Option<u16> {
        if self == Self::PaintOver {
            value.as_bool().map(u16::from)
        } else {
            value
                .as_f64()
                .filter(|v| (0.0..=f64::from(u16::MAX)).contains(v))
                .map(|v| v.round() as u16)
        }
    }

    pub fn encode(self, value: u16) -> Value {
        if self == Self::PaintOver {
            json!(value != 0)
        } else {
            json!(value)
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Control {
    pub current: u16,
    pub initial: u16,
    pub min: u16,
    pub max: u16,
    pub locked: bool,
}

#[derive(Clone)]
pub struct Settings {
    controls: BTreeMap<Key, Control>,
    pub jpeg_available: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            controls: BTreeMap::new(),
            jpeg_available: true,
        }
    }
}

impl Settings {
    pub fn announce(&mut self, payload: &Value) {
        let encoder = &payload["encoder"];
        self.jpeg_available = encoder["allowed"].as_array().map_or_else(
            || encoder["locked"].as_bool() != Some(true) || encoder["value"] == "jpeg",
            |allowed| allowed.iter().any(|value| value == "jpeg"),
        );
        for key in Key::ALL {
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

    pub fn request(&self, key: Key, value: u16) -> Option<u16> {
        self.get(key)
            .filter(|control| !control.locked && control.min != control.max)
            .map(|control| value.clamp(control.min, control.max))
    }
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

    pub fn apply(&mut self, change: ScreenChange) -> Option<Value> {
        match change {
            ScreenChange::Alignment(value) if self.alignment.is_some_and(|(_, locked)| !locked) => {
                self.alignment = Some((value, false));
                Some(json!({"force_aligned_resolution": value}))
            }
            ScreenChange::Dpi(value)
                if self.dpi_choices.len() > 1 && self.dpi_choices.contains(&value) =>
            {
                self.dpi = Some(value);
                Some(json!({"scaling_dpi": value}))
            }
            _ => None,
        }
    }
}

pub type Preferences = BTreeMap<Key, u16>;

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
        assert!(settings.jpeg_available);
        settings
            .announce(&json!({"encoder": {"value": "h264enc", "allowed": ["h264enc", "jpeg"]}}));
        assert!(settings.jpeg_available);
        settings.announce(&json!({"encoder": {"value": "h264enc", "allowed": ["h264enc"]}}));
        assert!(!settings.jpeg_available);
    }
}
