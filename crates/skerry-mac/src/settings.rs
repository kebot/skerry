use crate::clipboard::ClipboardSettings;
use crate::input::InputSettings;
use crate::launch_options::LaunchMemory;
use crate::shortcuts::ShortcutPolicy;
use crate::video::Codec;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
};

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
    /// The server's audio bitrate choices; `None` when it doesn't expose them.
    pub audio_bitrate: Option<AudioBitrate>,
    /// The server's `keyboard_shortcuts` (`ARCH-031`).
    pub keyboard_shortcuts: ShortcutPolicy,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            controls: BTreeMap::new(),
            codecs: Codec::ALL
                .into_iter()
                .filter(|codec| codec.supported())
                .collect(),
            audio_bitrate: None,
            keyboard_shortcuts: ShortcutPolicy::default(),
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
        self.audio_bitrate = AudioBitrate::from_server(&payload[AudioBitrate::WIRE]);
        self.keyboard_shortcuts = ShortcutPolicy::from_server(payload);
    }

    pub fn live(&mut self, payload: &Value) {
        for (&key, control) in &mut self.controls {
            if let Some(value) = key.read(&payload[key.wire()]) {
                control.current = value.clamp(control.min, control.max);
            }
        }
        if let Some(bitrate) = &mut self.audio_bitrate {
            bitrate.live(&payload[AudioBitrate::WIRE]);
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

/// The server's `audio_bitrate` enum (bps), as the web sidebar's Audio Bitrate reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioBitrate {
    pub value: u32,
    /// Ascending, without repeats, always holding `value`.
    pub choices: Vec<u32>,
    /// The server locked it.
    pub locked: bool,
}

impl AudioBitrate {
    pub const WIRE: &'static str = "audio_bitrate";

    /// `None` when the entry is absent or has no readable value. `allowed` arrives as
    /// strings in `server_settings`; numbers are read too.
    pub fn from_server(entry: &Value) -> Option<Self> {
        let value = bps(&entry["value"])?;
        let mut bitrate = Self {
            value,
            choices: entry["allowed"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(bps)
                .collect(),
            locked: entry["locked"] == true,
        };
        bitrate.insert(value);
        Some(bitrate)
    }

    /// Follows a live value (a number in `display_settings`, a string elsewhere). A value
    /// outside the list joins it, as Selkies does, rather than being clamped.
    pub fn live(&mut self, value: &Value) {
        if let Some(value) = bps(value) {
            self.value = value;
            self.insert(value);
        }
    }

    /// Whether the person can change it: not locked, and more than one choice.
    pub fn editable(&self) -> bool {
        !self.locked && self.choices.len() > 1
    }

    /// `bps` if it may be requested now: a listed value other than the current one.
    pub fn request(&self, bps: u32) -> Option<u32> {
        (self.editable() && bps != self.value && self.choices.contains(&bps)).then_some(bps)
    }

    fn insert(&mut self, value: u32) {
        self.choices.push(value);
        self.choices.sort_unstable();
        self.choices.dedup();
    }
}

fn bps(value: &Value) -> Option<u32> {
    value
        .as_u64()
        .or_else(|| value.as_str()?.trim().parse().ok())
        .and_then(|bps| u32::try_from(bps).ok())
        .filter(|bps| *bps > 0)
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

/// Which Session Controls sections are open. The default matches a fresh panel: Screen open,
/// the rest closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ControlsSections {
    pub input: bool,
    pub shortcuts: bool,
    pub clipboard: bool,
    pub screen: bool,
    pub video: bool,
    pub audio: bool,
    pub stats: bool,
}

impl Default for ControlsSections {
    fn default() -> Self {
        Self {
            input: false,
            shortcuts: false,
            clipboard: false,
            screen: true,
            video: false,
            audio: false,
            stats: false,
        }
    }
}

impl ControlsSections {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// In Session Controls order: Input, Shortcuts, Clipboard, Screen, Video, Audio, Stats.
    pub fn to_array(self) -> [bool; 7] {
        [
            self.input,
            self.shortcuts,
            self.clipboard,
            self.screen,
            self.video,
            self.audio,
            self.stats,
        ]
    }

    pub fn from_array(
        [input, shortcuts, clipboard, screen, video, audio, stats]: [bool; 7],
    ) -> Self {
        Self {
            input,
            shortcuts,
            clipboard,
            screen,
            video,
            audio,
            stats,
        }
    }
}

/// The most apps the Recent row remembers, as SealSkin's web launcher (`RECENT_MAX`).
pub const RECENT_MAX: usize = 8;
/// The most ids one Library list keeps; past it the oldest go first.
pub const LIST_CAP: usize = 512;

/// How the person organized this server's Library (`ARCH-027`). Kept only here: SealSkin
/// 0.5 has no per-user API for it, and its web launcher keeps the same lists in the browser.
/// Ids of apps the server no longer lists stay (an app can come back) and are skipped when
/// drawing.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct LibraryOrganization {
    /// App ids, oldest favorite first.
    #[serde(
        default,
        deserialize_with = "capped_oldest_first",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub favorites: Vec<String>,
    #[serde(
        default,
        deserialize_with = "capped_oldest_first",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub hidden: Vec<String>,
    /// App ids, newest launch first, at most [`RECENT_MAX`].
    #[serde(
        default,
        deserialize_with = "capped_newest_first",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub recent: Vec<String>,
}

/// The ids in a saved list: entries that aren't strings are dropped one by one (a list
/// that isn't a list reads as empty), and duplicates go.
fn read_ids<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Vec<String>, D::Error> {
    let value = Value::deserialize(deserializer)?;
    let mut seen = std::collections::HashSet::new();
    Ok(value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|id| id.as_str().map(str::to_string))
        .filter(|id| seen.insert(id.clone()))
        .collect())
}

/// Favorites and hidden apps, capped on load at the newest [`LIST_CAP`].
fn capped_oldest_first<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<String>, D::Error> {
    let mut ids = read_ids(deserializer)?;
    if ids.len() > LIST_CAP {
        ids.drain(..ids.len() - LIST_CAP);
    }
    Ok(ids)
}

/// Recents, capped on load at the newest [`RECENT_MAX`].
fn capped_newest_first<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<String>, D::Error> {
    let mut ids = read_ids(deserializer)?;
    ids.truncate(RECENT_MAX);
    Ok(ids)
}

impl LibraryOrganization {
    pub fn is_empty(&self) -> bool {
        self.favorites.is_empty() && self.hidden.is_empty() && self.recent.is_empty()
    }

    pub fn is_favorite(&self, app_id: &str) -> bool {
        self.favorites.iter().any(|id| id == app_id)
    }

    pub fn is_hidden(&self, app_id: &str) -> bool {
        self.hidden.iter().any(|id| id == app_id)
    }

    /// Adds `app_id` at the end of Favorites, or removes it. Setting, not toggling, so the
    /// write queue can apply the same change to its copy and to the file.
    pub fn set_favorite(&mut self, app_id: &str, favorite: bool) {
        set_member(&mut self.favorites, app_id, favorite);
    }

    pub fn set_hidden(&mut self, app_id: &str, hidden: bool) {
        set_member(&mut self.hidden, app_id, hidden);
    }

    /// Moves `app_id` to the front of Recent, keeping the newest [`RECENT_MAX`].
    pub fn record_launch(&mut self, app_id: &str) {
        self.recent.retain(|id| id != app_id);
        self.recent.insert(0, app_id.to_string());
        self.recent.truncate(RECENT_MAX);
    }
}

/// Adds `app_id` at the end of `list` or removes it, dropping duplicates and, past
/// [`LIST_CAP`], the oldest ids.
fn set_member(list: &mut Vec<String>, app_id: &str, member: bool) {
    let mut seen = std::collections::HashSet::new();
    list.retain(|id| id != app_id && seen.insert(id.clone()));
    if member {
        list.push(app_id.to_string());
    }
    if list.len() > LIST_CAP {
        list.drain(..list.len() - LIST_CAP);
    }
}

/// The audio choices saved for a server.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioPreferences {
    /// The chosen output device's raw Core Audio UID (`kAudioDevicePropertyDeviceUID`, not
    /// cpal's `coreaudio:` form); `None` follows the Mac's default output.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_device: Option<String>,
    /// The chosen device's last-seen name, to label it while it isn't connected.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_device_name: Option<String>,
    /// The audio bitrate (bps) last picked, re-requested when a session starts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bitrate: Option<u32>,
}

impl AudioPreferences {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Clone, Default, Deserialize, Serialize)]
pub struct Preferences {
    #[serde(default)]
    pub codec: Codec,
    #[serde(
        default,
        deserialize_with = "or_default",
        skip_serializing_if = "InputSettings::is_default"
    )]
    pub input: InputSettings,
    #[serde(
        default,
        deserialize_with = "or_default",
        skip_serializing_if = "ClipboardSettings::is_default"
    )]
    pub clipboard: ClipboardSettings,
    /// The Launch Dialog options each app last launched with, by app id.
    #[serde(
        default,
        deserialize_with = "or_default",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub launch: BTreeMap<String, LaunchMemory>,
    /// Which Session Controls sections were open last on this server.
    #[serde(
        default,
        deserialize_with = "or_default",
        skip_serializing_if = "ControlsSections::is_default"
    )]
    pub controls: ControlsSections,
    /// Favorites, hidden apps, and recents (`ARCH-027`); never written while empty.
    #[serde(
        default,
        deserialize_with = "or_default",
        skip_serializing_if = "LibraryOrganization::is_empty"
    )]
    pub library: LibraryOrganization,
    #[serde(default, skip_serializing_if = "AudioPreferences::is_default")]
    pub audio: AudioPreferences,
    /// **Mac shortcuts active** as the person set it; `None` follows the server (`ARCH-031`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mac_shortcuts: Option<bool>,
    /// **Show over the stream**: the stats strip (`ARCH-031`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stats_strip: bool,
    #[serde(flatten, deserialize_with = "lenient_video")]
    pub video: VideoSettings,
}

/// Reads the flattened video map, skipping keys this build doesn't know. (Files are read
/// field by field in `read_file`; this keeps `Preferences`' own `Deserialize` lenient too.)
fn lenient_video<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<VideoSettings, D::Error> {
    let raw = BTreeMap::<String, Value>::deserialize(deserializer)?;
    Ok(raw
        .into_iter()
        .filter_map(|(name, value)| {
            let key = serde_json::from_value::<Key>(Value::String(name)).ok()?;
            Some((key, serde_json::from_value::<u32>(value).ok()?))
        })
        .collect())
}

/// Reads a field, or its default when the value has another shape (a newer build's, or a
/// hand edit), so one bad field never fails the whole `Preferences`.
fn or_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Default + serde::de::DeserializeOwned,
{
    let value = Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).unwrap_or_default())
}

/// Where `server`'s preferences live. Writes go only through `persist::WriteQueue`.
pub(crate) fn path(server: &str) -> Option<PathBuf> {
    Some(
        PathBuf::from(std::env::var_os("HOME")?)
            .join("Library/Application Support/SealSkin Native")
            .join(format!("{server}.json")),
    )
}

/// Re-reads the saved preferences, applies `change`, and saves them, so that windows
/// changing different preferences never overwrite each other's (`ARCH-022`). Blocking:
/// only `persist`'s write queue calls it, so writes never race each other.
///
/// Top-level keys this build doesn't know (a newer build's) are written back unchanged. A
/// file that doesn't parse, or holds a value this build can't read, is first copied to
/// `<file>.bak-<unix seconds>`, so the defaults that replace the bad values lose nothing.
pub(crate) fn update_at(path: &Path, change: impl FnOnce(&mut Preferences)) -> io::Result<()> {
    let Loaded {
        mut preferences,
        extra,
        damaged,
    } = read_file(path);
    if damaged && path.exists() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs());
        std::fs::copy(path, format!("{}.bak-{stamp}", path.display()))?;
    }
    change(&mut preferences);
    let mut saved = match serde_json::to_value(&preferences)? {
        Value::Object(map) => map,
        _ => serde_json::Map::new(),
    };
    for (name, value) in extra {
        saved.entry(name).or_insert(value);
    }
    save_to(path, &Value::Object(saved))
}

/// The saved preferences at `path`; defaults for whatever is missing or unreadable.
pub(crate) fn load_at(path: &Path) -> Preferences {
    read_file(path).preferences
}

/// A preferences file as this build reads it.
struct Loaded {
    preferences: Preferences,
    /// Top-level keys this build doesn't know, kept to write back.
    extra: serde_json::Map<String, Value>,
    /// The file exists but didn't parse, or a value in it couldn't be read.
    damaged: bool,
}

/// Reads the file field by field, so one bad value costs only that value.
fn read_file(path: &Path) -> Loaded {
    let mut loaded = Loaded {
        preferences: Preferences::default(),
        extra: serde_json::Map::new(),
        damaged: false,
    };
    let map = match std::fs::read(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return loaded,
        Err(_) => None,
        Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
            Ok(Value::Object(map)) => Some(map),
            _ => None,
        },
    };
    let Some(map) = map else {
        loaded.damaged = true;
        return loaded;
    };
    let prefs = &mut loaded.preferences;
    for (name, value) in map {
        let read = match name.as_str() {
            "codec" => field(value, &mut prefs.codec),
            "input" => field(value, &mut prefs.input),
            "clipboard" => field(value, &mut prefs.clipboard),
            "launch" => field(value, &mut prefs.launch),
            "controls" => field(value, &mut prefs.controls),
            "library" => field(value, &mut prefs.library),
            "audio" => field(value, &mut prefs.audio),
            "mac_shortcuts" => field(value, &mut prefs.mac_shortcuts),
            "stats_strip" => field(value, &mut prefs.stats_strip),
            _ => match serde_json::from_value::<Key>(Value::String(name.clone())) {
                Ok(key) => serde_json::from_value::<u32>(value)
                    .map(|number| {
                        prefs.video.insert(key, number);
                    })
                    .is_ok(),
                Err(_) => {
                    loaded.extra.insert(name, value);
                    true
                }
            },
        };
        loaded.damaged |= !read;
    }
    loaded
}

/// Reads one field into `slot`; `false` (and the default kept) when it can't be read.
fn field<T: serde::de::DeserializeOwned>(value: Value, slot: &mut T) -> bool {
    serde_json::from_value(value)
        .map(|read| *slot = read)
        .is_ok()
}

fn save_to(path: &Path, preferences: &Value) -> io::Result<()> {
    let dir = path.parent().expect("preferences directory");
    std::fs::create_dir_all(dir)?;
    let mut file = tempfile::NamedTempFile::new_in(dir)?;
    serde_json::to_writer(&mut file, preferences)?;
    file.as_file().sync_all()?;
    file.persist(path)?;
    // The rename is durable only once the folder is synced.
    std::fs::File::open(dir)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn library_organization_is_absent_from_old_files_and_never_written_empty() {
        let prefs: Preferences = serde_json::from_value(json!({"jpeg": 45})).unwrap();
        assert!(prefs.library.is_empty());
        let saved = serde_json::to_value(&prefs).unwrap();
        assert!(saved.get("library").is_none(), "{saved}");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.json");
        update_at(&path, |p| p.library.set_favorite("zen", true)).unwrap();
        update_at(&path, |p| p.library.set_favorite("zen", false)).unwrap();
        let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert!(saved.get("library").is_none(), "{saved}");
    }

    #[test]
    fn library_organization_round_trips_with_launch_and_video_preferences() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.json");
        std::fs::write(
            &path,
            r#"{"jpeg": 45, "launch": {"zen": {"gpu": "intel-gpu"}}}"#,
        )
        .unwrap();
        update_at(&path, |p| {
            p.library.set_favorite("zen", true);
            p.library.set_hidden("krita", true);
            p.library.record_launch("obsidian");
        })
        .unwrap();
        let saved = load_at(&path);
        assert_eq!(saved.library.favorites, ["zen"]);
        assert_eq!(saved.library.hidden, ["krita"]);
        assert_eq!(saved.library.recent, ["obsidian"]);
        assert!(saved.launch.contains_key("zen"));
        assert_eq!(saved.video.get(&Key::Jpeg), Some(&45));
        assert_eq!(saved.video.len(), 1);
        // A clean file leaves no backup.
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn unknown_top_level_keys_do_not_reset_preferences() {
        let prefs: Preferences = serde_json::from_value(
            json!({"codec": "hevc", "jpeg": 45, "sidebar": {"width": 3}, "fps": "fast"}),
        )
        .unwrap();
        assert_eq!(prefs.codec, Codec::Hevc);
        assert_eq!(prefs.video.get(&Key::Jpeg), Some(&45));
        assert_eq!(prefs.video.len(), 1);
    }

    #[test]
    fn a_bad_library_entry_costs_only_that_entry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.json");
        std::fs::write(
            &path,
            r#"{"library": {"favorites": ["a", 5, "b", "a"], "hidden": ["z"], "recent": "x"}}"#,
        )
        .unwrap();
        let loaded = load_at(&path);
        assert_eq!(loaded.library.favorites, ["a", "b"]);
        assert_eq!(loaded.library.hidden, ["z"]);
        assert!(loaded.library.recent.is_empty());
    }

    #[test]
    fn library_lists_are_capped_on_load() {
        let ids: Vec<String> = (0..600).map(|n| format!("app{n}")).collect();
        let prefs: Preferences = serde_json::from_value(
            json!({"library": {"favorites": ids, "hidden": ids, "recent": ids}}),
        )
        .unwrap();
        assert_eq!(prefs.library.favorites.len(), LIST_CAP);
        assert_eq!(prefs.library.favorites[0], "app88");
        assert_eq!(prefs.library.hidden.len(), LIST_CAP);
        assert_eq!(prefs.library.recent, ids[..RECENT_MAX]);
    }

    #[test]
    fn a_malformed_library_keeps_every_other_preference() {
        for library in [json!(5), json!({"favorites": [1]}), json!({"hidden": "x"})] {
            let raw = json!({"codec": "hevc", "jpeg": 45, "library": library,
                "launch": {"zen": {"gpu": "intel-gpu"}}, "controls": {"video": true}});
            let prefs: Preferences = serde_json::from_value(raw.clone()).unwrap();
            assert_eq!(prefs.codec, Codec::Hevc);
            assert!(prefs.library.is_empty());
            assert!(prefs.launch.contains_key("zen"));
            assert!(prefs.controls.video);
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("server.json");
            std::fs::write(&path, raw.to_string()).unwrap();
            let loaded = load_at(&path);
            assert_eq!(loaded.codec, Codec::Hevc);
            assert_eq!(loaded.video.get(&Key::Jpeg), Some(&45));
            assert!(loaded.launch.contains_key("zen"));
            assert!(loaded.library.is_empty());
        }
    }

    #[test]
    fn unknown_keys_survive_a_round_trip_through_update() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.json");
        std::fs::write(&path, r#"{"jpeg": 45, "future": [1, 2], "theme": "plum"}"#).unwrap();
        update_at(&path, |p| p.library.set_favorite("zen", true)).unwrap();
        update_at(&path, |p| p.library.record_launch("zen")).unwrap();
        let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["future"], json!([1, 2]));
        assert_eq!(saved["theme"], json!("plum"));
        assert_eq!(
            saved["library"],
            json!({"favorites": ["zen"], "recent": ["zen"]})
        );
    }

    #[test]
    fn favorite_saved_by_launcher_survives_a_session_windows_codec_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.json");
        update_at(&path, |p| p.library.set_favorite("zen", true)).unwrap();
        update_at(&path, |p| p.codec = Codec::Hevc).unwrap();
        let saved = load_at(&path);
        assert_eq!(saved.codec, Codec::Hevc);
        assert!(saved.library.is_favorite("zen"));
    }

    #[test]
    fn recents_keep_the_newest_eight_without_duplicates() {
        let mut library = LibraryOrganization::default();
        for id in ["a", "b", "c", "d", "e", "f", "g", "h", "i", "b"] {
            library.record_launch(id);
        }
        assert_eq!(library.recent, ["b", "i", "h", "g", "f", "e", "d", "c"]);
    }

    #[test]
    fn toggles_deduplicate_and_cap_each_list() {
        let mut library = LibraryOrganization {
            favorites: vec!["a".into(), "a".into(), "b".into()],
            ..LibraryOrganization::default()
        };
        library.set_favorite("c", true);
        library.set_favorite("c", true);
        assert_eq!(library.favorites, ["a", "b", "c"]);
        library.set_favorite("a", false);
        assert_eq!(library.favorites, ["b", "c"]);
        for n in 0..LIST_CAP + 10 {
            library.set_hidden(&format!("app{n}"), true);
        }
        assert_eq!(library.hidden.len(), LIST_CAP);
        // The oldest go first.
        assert_eq!(library.hidden[0], "app10");
        assert!(library.is_hidden(&format!("app{}", LIST_CAP + 9)));
    }

    #[test]
    fn controls_sections_default_is_not_written() {
        let saved = serde_json::to_value(Preferences::default()).unwrap();
        assert!(saved.get("controls").is_none(), "{saved}");
    }

    #[test]
    fn controls_sections_round_trip_beside_video_settings() {
        let mut prefs: Preferences = serde_json::from_value(json!({"jpeg": 45})).unwrap();
        prefs.controls =
            ControlsSections::from_array([true, false, false, false, true, true, false]);
        let saved = serde_json::to_value(&prefs).unwrap();
        let restored: Preferences = serde_json::from_value(saved).unwrap();
        // `controls` is read as the struct, not swallowed by the flattened video map.
        assert_eq!(
            restored.controls.to_array(),
            [true, false, false, false, true, true, false]
        );
        assert_eq!(restored.video.get(&Key::Jpeg), Some(&45));
        assert_eq!(restored.video.len(), 1);
    }

    #[test]
    fn controls_sections_round_trip_with_shortcuts() {
        for bits in 0..128u8 {
            let open: [bool; 7] = std::array::from_fn(|i| bits & (1 << i) != 0);
            assert_eq!(ControlsSections::from_array(open).to_array(), open);
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.json");
        std::fs::write(&path, r#"{"controls": {"stats": true}}"#).unwrap();
        let old = load_at(&path).controls;
        assert!(old.stats && !old.shortcuts && old.screen);
    }

    #[test]
    fn shortcut_and_strip_preferences_round_trip() {
        let saved = serde_json::to_value(Preferences::default()).unwrap();
        assert!(saved.get("mac_shortcuts").is_none(), "{saved}");
        assert!(saved.get("stats_strip").is_none(), "{saved}");

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.json");
        update_at(&path, |p| {
            p.mac_shortcuts = Some(false);
            p.stats_strip = true;
        })
        .unwrap();
        let loaded = load_at(&path);
        assert_eq!(loaded.mac_shortcuts, Some(false));
        assert!(loaded.stats_strip);

        // A value of the wrong shape loads as the default and the file is backed up.
        std::fs::write(&path, r#"{"stats_strip": "yes", "mac_shortcuts": true}"#).unwrap();
        let loaded = read_file(&path);
        assert!(loaded.damaged);
        assert!(!loaded.preferences.stats_strip);
        assert_eq!(loaded.preferences.mac_shortcuts, Some(true));
        update_at(&path, |p| p.codec = Codec::Hevc).unwrap();
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    #[test]
    fn announce_reads_keyboard_shortcuts() {
        let mut settings = Settings::default();
        assert_eq!(settings.keyboard_shortcuts, ShortcutPolicy::default());
        settings.announce(&json!({"keyboard_shortcuts": {"value": false, "locked": true}}));
        assert_eq!(
            settings.keyboard_shortcuts,
            ShortcutPolicy {
                value: false,
                locked: true
            }
        );
    }

    #[test]
    fn unknown_preferences_from_a_newer_build_dont_wipe_the_file() {
        let prefs: Preferences = serde_json::from_value(
            json!({"codec": "hevc", "jpeg": 45, "future": {"favorites": ["zen"]}}),
        )
        .unwrap();
        assert_eq!(prefs.video.get(&Key::Jpeg), Some(&45));
        assert_eq!(prefs.video.len(), 1);
    }

    #[test]
    fn unknown_top_level_keys_survive_a_rewrite() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.json");
        std::fs::write(&path, r#"{"jpeg": 45, "future": {"favorites": ["zen"]}}"#).unwrap();
        update_at(&path, |p| p.codec = Codec::Hevc).unwrap();
        let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["future"], json!({"favorites": ["zen"]}));
        assert_eq!(saved["jpeg"], json!(45));
        assert_eq!(saved["codec"], json!("hevc"));
        // A clean file leaves no backup.
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn an_unreadable_value_is_backed_up_and_the_rest_survives() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.json");
        let original = r#"{"codec": "h266", "jpeg": 45, "launch": {"zen": {"gpu": "intel-gpu"}}}"#;
        std::fs::write(&path, original).unwrap();
        update_at(&path, |p| p.controls.video = true).unwrap();
        let saved = load_at(&path);
        assert_eq!(saved.codec, Codec::default());
        assert_eq!(saved.video.get(&Key::Jpeg), Some(&45));
        assert!(saved.launch.contains_key("zen"));
        assert!(saved.controls.video);
        let backups: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("server.json.bak-")
            })
            .collect();
        assert_eq!(backups.len(), 1);
        assert_eq!(
            std::fs::read_to_string(backups[0].path()).unwrap(),
            original
        );
    }

    #[test]
    fn a_file_that_doesnt_parse_is_backed_up_before_the_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.json");
        std::fs::write(&path, r#"{"codec": "hevc", "jpe"#).unwrap();
        update_at(&path, |p| p.codec = Codec::Av1).unwrap();
        assert_eq!(load_at(&path).codec, Codec::Av1);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    #[test]
    fn updates_keep_what_other_windows_saved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.json");
        update_at(&path, |p| p.codec = Codec::Hevc).unwrap();
        update_at(&path, |p| {
            p.video.insert(Key::Fps, 30);
        })
        .unwrap();
        let saved = load_at(&path);
        assert_eq!(saved.codec, Codec::Hevc);
        assert_eq!(saved.video.get(&Key::Fps), Some(&30));
    }

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
    /// The `audio_bitrate` entry Selkies 715df7c sends in `server_settings`.
    fn selkies_audio_bitrate() -> Value {
        json!({
            "value": "128000",
            "overridden": false,
            "allowed": ["32000", "48000", "64000", "96000", "128000", "192000", "256000",
                        "320000", "384000", "510000"]
        })
    }

    #[test]
    fn audio_bitrate_parses_server_allowed_strings() {
        let bitrate = AudioBitrate::from_server(&selkies_audio_bitrate()).unwrap();
        assert_eq!(bitrate.value, 128_000);
        assert_eq!(
            bitrate.choices,
            [
                32_000, 48_000, 64_000, 96_000, 128_000, 192_000, 256_000, 320_000, 384_000,
                510_000
            ]
        );
        assert!(bitrate.editable());
        let mut settings = Settings::default();
        settings.announce(&json!({"audio_bitrate": selkies_audio_bitrate()}));
        assert_eq!(settings.audio_bitrate, Some(bitrate));
    }

    #[test]
    fn audio_bitrate_hidden_only_when_absent() {
        assert_eq!(AudioBitrate::from_server(&Value::Null), None);
        assert_eq!(AudioBitrate::from_server(&json!({"value": "fast"})), None);
        let mut settings = Settings::default();
        settings.announce(&json!({}));
        assert_eq!(settings.audio_bitrate, None);
    }

    #[test]
    fn audio_bitrate_disabled_when_locked_or_single() {
        let mut locked = selkies_audio_bitrate();
        locked["locked"] = json!(true);
        let locked = AudioBitrate::from_server(&locked).unwrap();
        assert!(!locked.editable());
        assert_eq!(locked.request(96_000), None);
        let single =
            AudioBitrate::from_server(&json!({"value": "96000", "allowed": ["96000"]})).unwrap();
        assert_eq!((single.value, single.editable()), (96_000, false));
        let bare = AudioBitrate::from_server(&json!({"value": 64000})).unwrap();
        assert_eq!(bare.choices, [64_000]);
        assert!(!bare.editable());
    }

    #[test]
    fn audio_bitrate_request_accepts_only_listed_values() {
        let bitrate = AudioBitrate::from_server(&selkies_audio_bitrate()).unwrap();
        assert_eq!(bitrate.request(96_000), Some(96_000));
        assert_eq!(bitrate.request(128_000), None, "already the value");
        assert_eq!(bitrate.request(100_000), None, "not offered");
    }

    #[test]
    fn audio_bitrate_follows_live_updates() {
        let mut settings = Settings::default();
        settings.announce(&json!({"audio_bitrate": selkies_audio_bitrate()}));
        settings.live(&json!({"audio_bitrate": "320000"}));
        assert_eq!(settings.audio_bitrate.as_ref().unwrap().value, 320_000);
        settings.live(&json!({"framerate": 30}));
        assert_eq!(settings.audio_bitrate.as_ref().unwrap().value, 320_000);
    }

    #[test]
    fn audio_bitrate_live_accepts_numeric_display_settings() {
        let mut settings = Settings::default();
        settings.announce(&json!({"audio_bitrate": selkies_audio_bitrate()}));
        settings.live(&json!({"audio_bitrate": 64000}));
        assert_eq!(settings.audio_bitrate.unwrap().value, 64_000);
    }

    #[test]
    fn audio_bitrate_off_list_value_is_shown_not_clamped() {
        let mut bitrate = AudioBitrate::from_server(&selkies_audio_bitrate()).unwrap();
        bitrate.live(&json!(100_000));
        assert_eq!(bitrate.value, 100_000);
        assert_eq!(&bitrate.choices[3..6], [96_000, 100_000, 128_000]);
        let operator =
            AudioBitrate::from_server(&json!({"value": "24000", "allowed": ["32000", "64000"]}))
                .unwrap();
        assert_eq!(operator.choices, [24_000, 32_000, 64_000]);
    }

    #[test]
    fn audio_preferences_round_trip_and_old_files_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.json");
        // A file from before audio preferences: loads with the defaults, video untouched.
        std::fs::write(&path, r#"{"jpeg": 45, "controls": {"video": true}}"#).unwrap();
        let old = load_at(&path);
        assert!(old.audio.is_default());
        assert_eq!(old.video.get(&Key::Jpeg), Some(&45));
        update_at(&path, |p| {
            p.audio.output_device = Some("AppleUSBAudioEngine:1234".into());
            p.audio.output_device_name = Some("USB DAC".into());
            p.audio.bitrate = Some(96_000);
        })
        .unwrap();
        let saved = load_at(&path);
        assert_eq!(
            saved.audio.output_device.as_deref(),
            Some("AppleUSBAudioEngine:1234")
        );
        assert_eq!(saved.audio.output_device_name.as_deref(), Some("USB DAC"));
        assert_eq!(saved.audio.bitrate, Some(96_000));
        assert_eq!(saved.video.get(&Key::Jpeg), Some(&45));
        assert!(saved.controls.video);
        // Defaults are left out of the file.
        assert!(!serde_json::to_value(Preferences::default())
            .unwrap()
            .as_object()
            .unwrap()
            .contains_key("audio"));
    }

    #[test]
    fn controls_sections_without_audio_still_load() {
        // Written by a build whose Session Controls had no Audio card.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.json");
        std::fs::write(
            &path,
            r#"{"controls": {"input": true, "clipboard": false, "screen": false, "video": true, "stats": true}}"#,
        )
        .unwrap();
        let saved = load_at(&path);
        assert_eq!(
            saved.controls.to_array(),
            [true, false, false, false, true, false, true]
        );
    }

    #[test]
    fn audio_preferences_round_trip_beside_video_and_controls() {
        let mut prefs: Preferences = serde_json::from_value(json!({"bitrate": 8000})).unwrap();
        prefs.audio.bitrate = Some(320_000);
        prefs.controls.audio = true;
        let saved = serde_json::to_value(&prefs).unwrap();
        // Audio is its own struct: its `bitrate` never lands in the flattened video map.
        assert_eq!(saved["audio"], json!({"bitrate": 320000}));
        assert_eq!(saved["bitrate"], json!(8000));
        let restored: Preferences = serde_json::from_value(saved).unwrap();
        assert_eq!(restored.audio.bitrate, Some(320_000));
        assert!(restored.controls.audio);
        assert_eq!(restored.video.get(&Key::Bitrate), Some(&8000));
        assert_eq!(restored.video.len(), 1);
    }
}
