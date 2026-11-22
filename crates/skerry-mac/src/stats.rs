//! What the Stats section says (`ARCH-019`), worded by the rules of Selkies'
//! `selkies-web-core/lib/stream-stats-view.ts` for a WebSockets session: the
//! four status rows, the graphs' series, the figure tiles, the host meters, and
//! the plain-text report. Drawing is `skerry_ui::stats`.

use crate::transport::{StreamInfo, StreamSample};
use crate::video::Codec;
use skerry_ui::stats::{
    Graph, Meter, Series, StatRow, StatsProps, Status, StripFigure, StripProps, Tile,
};

/// Seconds of history kept for the graphs (the web's `HISTORY_MAX`).
pub const HISTORY_MAX: usize = 600;
/// Points a graph draws at most; a longer history is bucketed by maximum.
const GRAPH_POINTS: usize = 120;
/// How far above its CBR target the video rate may run before it warns.
const OVERSHOOT: f64 = 1.1;
/// How many samples (seconds) a measured time stands in for later ones.
const HOLD_SAMPLES: usize = 5;

fn joined<'a>(parts: impl IntoIterator<Item = &'a str>) -> String {
    parts
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

fn sentence(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

fn codec_name(codec: &str) -> String {
    match codec.to_ascii_lowercase().as_str() {
        "h264" => "H.264".into(),
        "h265" | "hevc" => "H.265".into(),
        "av1" => "AV1".into(),
        "vp8" => "VP8".into(),
        "vp9" => "VP9".into(),
        "jpeg" => "JPEG".into(),
        other => other.to_ascii_uppercase(),
    }
}

fn node_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or("")
}

/// A figure as the server sent it, to at most two decimals.
fn number(value: f64) -> String {
    let rounded = (value * 100.).round() / 100.;
    format!("{rounded}")
}

fn encoder_row(info: Option<&StreamInfo>) -> StatRow {
    let Some(info) = info else {
        return row("Encoder", Status::Neutral, "", "", "");
    };
    let video = info.codec != "jpeg";
    let fell = !info.hardware && info.hardware_expected;
    let format = format!(
        "{}{}{}",
        codec_name(&info.codec),
        match (video, info.fullcolor) {
            (false, _) => "",
            (true, true) => " 4:4:4",
            (true, false) => " 4:2:0",
        },
        if video && info.bit_depth == Some(10) {
            " 10-bit"
        } else {
            ""
        },
    );
    let detail = if info.hardware {
        joined([
            info.gpu.as_str(),
            node_name(&info.encode_node),
            info.driver.as_str(),
        ])
    } else if info.gpu_present == Some(false) {
        "No GPU exposed to the server".into()
    } else {
        String::new()
    };
    row(
        "Encoder",
        if info.hardware {
            Status::Good
        } else if fell {
            Status::Warn
        } else {
            Status::Neutral
        },
        &joined([
            info.encoder.as_str(),
            format.as_str(),
            if info.striped { "Striped" } else { "" },
        ]),
        &detail,
        &if fell {
            sentence(&info.encoder_reason)
        } else {
            String::new()
        },
    )
}

fn capture_row(info: Option<&StreamInfo>) -> StatRow {
    let Some(info) = info else {
        return row("Capture", Status::Neutral, "", "", "");
    };
    let wayland = info.backend == "wayland";
    let software = wayland && info.renderer == "pixman";
    let mut path = if info.zero_copy {
        "Zero-copy".to_string()
    } else {
        "Readback".to_string()
    };
    if !wayland {
        path.push_str(&format!(" ({})", info.capture));
    }
    let readback_to_gpu = !info.zero_copy && info.hardware && info.zero_copy_available;
    let rendered_in_software = software && info.hardware_expected;
    let detail = if !wayland {
        String::new()
    } else if software {
        "Pixman (software rendering)".into()
    } else {
        let gl = format!("GL on {}", node_name(&info.render_node));
        joined([gl.as_str(), info.render_gpu.as_str()])
    };
    row(
        "Capture",
        if info.zero_copy {
            Status::Good
        } else if readback_to_gpu || rendered_in_software {
            Status::Warn
        } else {
            Status::Neutral
        },
        &joined([if wayland { "Wayland" } else { "X11" }, path.as_str()]),
        &detail,
        &sentence(if rendered_in_software {
            &info.renderer_reason
        } else if readback_to_gpu {
            &info.capture_reason
        } else {
            ""
        }),
    )
}

/// Skerry decodes H.264, H.265, and AV1 in VideoToolbox, and JPEG on the CPU,
/// where no hardware path is expected, so software JPEG is neutral.
fn decoder_row(latest: Option<&StreamSample>) -> StatRow {
    let Some(sample) = latest else {
        return row("Decoder", Status::Neutral, "", "", "");
    };
    let Some(codec) = sample.client_codec else {
        return row("Decoder", Status::Neutral, "", "Waiting for video", "");
    };
    let hardware = codec != Codec::Jpeg;
    let resolution = format!("{}×{}", sample.width, sample.height);
    row(
        "Decoder",
        if hardware {
            Status::Good
        } else {
            Status::Neutral
        },
        if hardware { "Hardware" } else { "Software" },
        &joined([
            if hardware { "VideoToolbox" } else { "" },
            match codec {
                Codec::Jpeg => "JPEG",
                Codec::H264 => "H.264",
                Codec::Hevc => "H.265",
                Codec::Av1 => "AV1",
            },
            resolution.as_str(),
        ]),
        "",
    )
}

fn connection_row(latest: Option<&StreamSample>) -> StatRow {
    let throttled = latest
        .and_then(|sample| sample.server.as_ref())
        .is_some_and(|server| server.throttled);
    row(
        "Connection",
        if throttled {
            Status::Warn
        } else {
            Status::Neutral
        },
        "WebSockets",
        "",
        if throttled {
            "The server is holding frames back because this client is falling behind"
        } else {
            ""
        },
    )
}

fn row(label: &str, status: Status, value: &str, detail: &str, reason: &str) -> StatRow {
    StatRow {
        label: label.into(),
        status,
        value: value.into(),
        detail: detail.into(),
        reason: reason.into(),
    }
}

pub fn rows(info: Option<&StreamInfo>, latest: Option<&StreamSample>) -> Vec<StatRow> {
    vec![
        encoder_row(info),
        capture_row(info),
        decoder_row(latest),
        connection_row(latest),
    ]
}

/// A sample's figure by the web's key: the server's, or Skerry's own.
fn figure(sample: &StreamSample, key: &str) -> Option<f64> {
    match key {
        "decode_ms" => (sample.client_codec.is_some() && sample.decode_ms > 0.)
            .then(|| (sample.decode_ms * 100.).round() / 100.),
        "fps" => Some(sample.fps.round()),
        "mbps" => Some((sample.wire_mbps * 100.).round() / 100.),
        "rtt_ms" => sample
            .rtt_ms
            .map(|ms| (ms * 10.).round() / 10.)
            .or_else(|| sample.server.as_ref()?.get("rtt_ms")),
        _ => sample.server.as_ref()?.get(key),
    }
}

/// The tiles Skerry measures, in the web's WebSockets order, leaving out the
/// ones it does not (Arrival to screen, Audio buffer, Frames not shown).
const TILES: [(&str, &str, &str); 9] = [
    ("encode_ms", "Encode", "ms"),
    ("pipeline_ms", "Capture to encoded", "ms"),
    ("send_ms", "Capture to sent", "ms"),
    ("decode_ms", "Decode", "ms"),
    ("video_mbps", "Video rate", "Mbps"),
    ("peak_mbps", "Peak, 250 ms", "Mbps"),
    ("received_mb", "Received", "MB"),
    ("lost_frames", "Lost frames", ""),
    ("keyframe_requests", "Key frame requests", ""),
];

fn range_keys(key: &str) -> Option<(&'static str, &'static str)> {
    match key {
        "pipeline_ms" => Some(("pipeline_min_ms", "pipeline_max_ms")),
        "send_ms" => Some(("send_min_ms", "send_max_ms")),
        _ => None,
    }
}

fn tile(history: &[StreamSample], key: &str, label: &str, unit: &str) -> Tile {
    let dash = |value: &str| Tile {
        label: label.into(),
        value: value.into(),
        detail: String::new(),
        warn: false,
    };
    let Some(latest) = history.last() else {
        return dash("–");
    };
    let first = &history[0];
    let counted = match key {
        "received_mb" => Some((latest.received_bytes as f64 / 1e4).round() / 100.),
        "lost_frames" => Some(latest.dropped.saturating_sub(first.dropped) as f64),
        "keyframe_requests" => Some(latest.keyframe_requests as f64),
        _ => None,
    };
    // A time with nothing measured this second reads the last one measured
    // within `HOLD_SAMPLES` seconds, else idle where no frame moved.
    let shown = if counted.is_some() || figure(latest, key).is_some() {
        latest
    } else if unit == "ms" {
        match history
            .iter()
            .rev()
            .take(HOLD_SAMPLES + 1)
            .find(|sample| figure(sample, key).is_some())
        {
            Some(sample) => sample,
            None => {
                let encoded = figure(latest, "encoded_fps");
                let idle = encoded == Some(0.) || latest.fps == 0.;
                return dash(if idle { "idle" } else { "–" });
            }
        }
    } else {
        return dash("–");
    };
    let Some(value) = counted.or_else(|| figure(shown, key)) else {
        return dash("–");
    };
    let target = (key == "video_mbps")
        .then(|| figure(shown, "target_mbps"))
        .flatten()
        .filter(|target| *target > 0.);
    let detail = range_keys(key)
        .and_then(|(least, most)| {
            Some(format!(
                "{}–{}",
                number(figure(shown, least)?),
                number(figure(shown, most)?)
            ))
        })
        .unwrap_or_default();
    let mut text = number(value);
    if let Some(target) = target {
        text.push_str(&format!(" / {}", number(target)));
    }
    if !unit.is_empty() {
        text.push(' ');
        text.push_str(unit);
    }
    Tile {
        label: label.into(),
        value: text,
        detail,
        warn: target.is_some_and(|target| value > OVERSHOOT * target),
    }
}

pub fn tiles(history: &[StreamSample]) -> Vec<Tile> {
    TILES
        .iter()
        .map(|(key, label, unit)| tile(history, key, label, unit))
        .collect()
}

fn gib(bytes: f64) -> String {
    format!("{:.1} GiB", bytes / 1_073_741_824.)
}

/// The host meters: utilizations as bars, memories as their amounts, the GPU's
/// left out where the server reads no GPU.
pub fn meters(latest: Option<&StreamSample>) -> Vec<Meter> {
    let Some(server) = latest.and_then(|sample| sample.server.as_ref()) else {
        return Vec::new();
    };
    let Some(cpu) = server.get("cpu_percent") else {
        return Vec::new();
    };
    let share = |used: f64, total: f64| {
        if total > 0. {
            (100. * used / total).min(100.)
        } else {
            0.
        }
    };
    let used = |label: &str, percent: f64| {
        let percent = percent.min(100.);
        Meter {
            label: label.into(),
            percent: percent as f32,
            text: if percent < 10. {
                format!("{percent:.1}%")
            } else {
                format!("{}%", percent.round())
            },
            bar: true,
        }
    };
    let amounts = |label: &str, gotten: f64, total: f64| Meter {
        label: label.into(),
        percent: share(gotten, total) as f32,
        text: format!(
            "{} / {} · {}%",
            gib(gotten),
            gib(total),
            share(gotten, total).round()
        ),
        bar: false,
    };
    let gpu = server.get("gpu_percent");
    let mut meters = vec![used("CPU", cpu)];
    if let Some(gpu) = gpu {
        meters.push(used("GPU", gpu));
    }
    meters.push(amounts(
        "MEM",
        server.get("mem_used").unwrap_or(0.),
        server.get("mem_total").unwrap_or(0.),
    ));
    let vram_total = server.get("gpu_mem_total").unwrap_or(0.);
    if gpu.is_some() && vram_total > 0. {
        meters.push(amounts(
            "VRAM",
            server.get("gpu_mem_used").unwrap_or(0.),
            vram_total,
        ));
    }
    meters
}

/// One series of a history, bucketed down to `GRAPH_POINTS` by maximum,
/// since a graph of a rate is read for its peaks.
pub fn series_of(history: &[StreamSample], key: &str) -> Vec<f32> {
    let values: Vec<f32> = history
        .iter()
        .map(|sample| figure(sample, key).unwrap_or(0.) as f32)
        .collect();
    if values.len() <= GRAPH_POINTS {
        return values;
    }
    let size = values.len() as f32 / GRAPH_POINTS as f32;
    (0..GRAPH_POINTS)
        .map(|i| {
            let start = (i as f32 * size) as usize;
            let end = (((i + 1) as f32 * size) as usize).max(start + 1);
            values[start..end.min(values.len())]
                .iter()
                .copied()
                .fold(0., f32::max)
        })
        .collect()
}

/// The top of a graph's axis: the largest value with 10% headroom, never under `floor`.
fn axis_top(floor: f32, lists: &[&[f32]]) -> f32 {
    let most = lists
        .iter()
        .flat_map(|list| list.iter().copied())
        .fold(0., f32::max);
    floor.max((most * 1.1).ceil())
}

pub fn graphs(history: &[StreamSample], framerate: u32) -> Vec<Graph> {
    let fps = series_of(history, "fps");
    let encoded = series_of(history, "encoded_fps");
    let has_encoded = history
        .iter()
        .any(|sample| figure(sample, "encoded_fps").is_some());
    let mbps = series_of(history, "mbps");
    let rtt = series_of(history, "rtt_ms");
    let floor = if framerate > 0 { framerate as f32 } else { 60. };
    vec![
        Graph {
            label: "FPS".into(),
            unit: "fps".into(),
            max: axis_top(floor, &[&fps, &encoded]),
            series: if has_encoded {
                vec![
                    Series {
                        name: "Client".into(),
                        values: fps,
                    },
                    Series {
                        name: "Server".into(),
                        values: encoded,
                    },
                ]
            } else {
                vec![Series {
                    name: String::new(),
                    values: fps,
                }]
            },
        },
        Graph {
            label: "Bandwidth".into(),
            unit: "Mbps".into(),
            max: axis_top(1., &[&mbps]),
            series: vec![Series {
                name: String::new(),
                values: mbps,
            }],
        },
        Graph {
            label: "Latency".into(),
            unit: "ms".into(),
            max: axis_top(20., &[&rtt]),
            series: vec![Series {
                name: String::new(),
                values: rtt,
            }],
        },
    ]
}

pub fn props(info: Option<&StreamInfo>, history: &[StreamSample], framerate: u32) -> StatsProps {
    let latest = history.last();
    StatsProps {
        rows: rows(info, latest),
        graphs: graphs(history, framerate),
        tiles: tiles(history),
        meters: meters(latest),
    }
}

/// Why the session subscribes to the server's `stream_stats` (`ARCH-031`, amending
/// `ARCH-019`): while the Stats section is open **or** the strip is shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StatsInterest {
    section: bool,
    strip: bool,
}

impl StatsInterest {
    pub fn new(strip: bool) -> Self {
        Self {
            section: false,
            strip,
        }
    }

    pub fn wanted(&self) -> bool {
        self.section || self.strip
    }

    /// The Stats section opened or shut. `Some(wanted)` only when the subscription starts
    /// or stops: the caller then clears the history and tells the stream.
    pub fn set_section(&mut self, open: bool) -> Option<bool> {
        self.change(|interest| interest.section = open)
    }

    /// The strip was switched. As [`Self::set_section`].
    pub fn set_strip(&mut self, on: bool) -> Option<bool> {
        self.change(|interest| interest.strip = on)
    }

    fn change(&mut self, apply: impl FnOnce(&mut Self)) -> Option<bool> {
        let before = self.wanted();
        apply(self);
        (self.wanted() != before).then_some(self.wanted())
    }
}

/// Samples the strip reads: the latest, and enough before it for a held time.
pub const STRIP_SAMPLES: usize = HOLD_SAMPLES + 1;

/// A millisecond figure as the strip reads it: one decimal under 10, whole above.
fn strip_ms(ms: f64) -> String {
    if ms < 10. {
        format!("{} ms", number((ms * 10.).round() / 10.))
    } else {
        format!("{} ms", ms.round())
    }
}

/// The strip over the stream (the web's `streamStrip` for WebSockets): the four status
/// marks, then FPS, Video rate, Capture to sent, and Latency. `history` needs only its last
/// [`STRIP_SAMPLES`] samples.
pub fn strip(info: Option<&StreamInfo>, history: &[StreamSample]) -> StripProps {
    let latest = history.last();
    let figure_of = |value: String, caption: &str, warn: bool| StripFigure {
        value,
        caption: caption.into(),
        warn,
    };
    let fps = latest
        .and_then(|sample| figure(sample, "fps"))
        .map_or_else(|| "–".to_string(), |fps| format!("{fps} fps"));
    let rate = tile(history, "video_mbps", "Video rate", "Mbps");
    let send = tile(history, "send_ms", "Capture to sent", "ms");
    let latency = latest
        .and_then(|sample| figure(sample, "rtt_ms"))
        .map_or_else(|| "–".to_string(), strip_ms);
    StripProps {
        marks: rows(info, latest)
            .into_iter()
            .map(|row| row.status)
            .collect(),
        figures: vec![
            figure_of(fps, "FPS", false),
            figure_of(rate.value, "Video rate", rate.warn),
            figure_of(send.value, "Capture to sent", false),
            figure_of(latency, "Latency", false),
        ],
    }
}

/// The session as plain text for an issue or a chat (the web's `streamReport`).
pub fn report(info: Option<&StreamInfo>, history: &[StreamSample]) -> String {
    let latest = history.last();
    let rows = rows(info, latest);
    let mut lines: Vec<String> = rows
        .iter()
        .map(|row| {
            format!(
                "{}: {}{}",
                row.label.to_lowercase(),
                joined([row.value.as_str(), row.detail.as_str()]),
                if row.status == Status::Warn {
                    " [!]"
                } else {
                    ""
                }
            )
        })
        .collect();
    lines.extend(
        rows.iter()
            .filter(|row| !row.reason.is_empty())
            .map(|row| format!("  {}", row.reason)),
    );
    if let Some(latest) = latest {
        let shown: Vec<String> = ["fps", "encoded_fps", "mbps", "rtt_ms"]
            .into_iter()
            .filter_map(|key| Some(format!("{key} {}", number(figure(latest, key)?))))
            .collect();
        lines.push(shown.join(" · "));
        lines.extend(tiles(history).into_iter().map(|tile| {
            format!(
                "{}: {}{}{}",
                tile.label,
                tile.value,
                if tile.detail.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", tile.detail)
                },
                if tile.warn { " [!]" } else { "" }
            )
        }));
        lines.extend(
            meters(Some(latest))
                .into_iter()
                .map(|meter| format!("{}: {}", meter.label.to_lowercase(), meter.text)),
        );
    }
    lines.push(format!("SealSkin Native {}", env!("CARGO_PKG_VERSION")));
    lines.retain(|line| !line.is_empty());
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::ServerFigures;

    fn sample(fps: f64, server: &[(&str, f64)]) -> StreamSample {
        StreamSample {
            width: 1920,
            height: 1080,
            fps,
            wire_mbps: 4.0,
            decode_ms: 0.8,
            dropped: 0,
            client_codec: Some(Codec::Hevc),
            rtt_ms: Some(9.0),
            server: (!server.is_empty()).then(|| ServerFigures {
                numbers: server.iter().map(|(k, v)| ((*k).into(), *v)).collect(),
                throttled: false,
            }),
            received_bytes: 0,
            keyframe_requests: 0,
        }
    }

    fn info() -> StreamInfo {
        StreamInfo {
            backend: "x11".into(),
            capture: "xshm".into(),
            encoder: "x264".into(),
            codec: "h264".into(),
            ..StreamInfo::default()
        }
    }

    #[test]
    fn rows_warn_only_where_the_session_fell_short() {
        let neutral = rows(Some(&info()), Some(&sample(60., &[])));
        assert_eq!(neutral[0].status, Status::Neutral);
        assert_eq!(neutral[0].value, "x264 · H.264 4:2:0");
        assert_eq!(neutral[1].value, "X11 · Readback (xshm)");
        assert_eq!(neutral[1].status, Status::Neutral);
        assert_eq!(neutral[2].status, Status::Good);
        assert_eq!(neutral[2].detail, "VideoToolbox · H.265 · 1920×1080");
        assert_eq!(neutral[3].status, Status::Neutral);

        let fell = StreamInfo {
            hardware_expected: true,
            encoder_reason: "nvenc session limit reached".into(),
            ..info()
        };
        let row = &rows(Some(&fell), None)[0];
        assert_eq!(row.status, Status::Warn);
        assert_eq!(row.reason, "Nvenc session limit reached");

        let readback = StreamInfo {
            hardware: true,
            zero_copy_available: true,
            capture_reason: "dmabuf import failed".into(),
            gpu: "Intel Arc".into(),
            encode_node: "/dev/dri/renderD128".into(),
            ..info()
        };
        let rows = rows(Some(&readback), None);
        assert_eq!(rows[0].status, Status::Good);
        assert_eq!(rows[0].detail, "Intel Arc · renderD128");
        assert_eq!(rows[1].status, Status::Warn);
        assert_eq!(rows[1].reason, "Dmabuf import failed");

        let mut throttled = sample(60., &[("cpu_percent", 5.)]);
        throttled.server.as_mut().unwrap().throttled = true;
        assert_eq!(connection_row(Some(&throttled)).status, Status::Warn);
    }

    #[test]
    fn jpeg_decoding_on_the_cpu_is_neutral() {
        let mut jpeg = sample(30., &[]);
        jpeg.client_codec = Some(Codec::Jpeg);
        let row = decoder_row(Some(&jpeg));
        assert_eq!(
            (row.status, row.value.as_str()),
            (Status::Neutral, "Software")
        );
    }

    #[test]
    fn a_time_holds_for_five_seconds_then_reads_idle() {
        let mut history = vec![sample(60., &[("encode_ms", 3.5)])];
        for _ in 0..HOLD_SAMPLES {
            history.push(sample(0., &[("encoded_fps", 0.)]));
        }
        assert_eq!(tile(&history, "encode_ms", "Encode", "ms").value, "3.5 ms");
        history.push(sample(0., &[("encoded_fps", 0.)]));
        assert_eq!(tile(&history, "encode_ms", "Encode", "ms").value, "idle");
        history.push(sample(60., &[("encoded_fps", 60.)]));
        assert_eq!(tile(&history, "encode_ms", "Encode", "ms").value, "–");
    }

    #[test]
    fn video_rate_warns_over_its_target_and_ranges_show() {
        let history = [sample(
            60.,
            &[
                ("video_mbps", 8.9),
                ("target_mbps", 8.0),
                ("pipeline_ms", 6.0),
                ("pipeline_min_ms", 4.25),
                ("pipeline_max_ms", 9.0),
            ],
        )];
        let rate = tile(&history, "video_mbps", "Video rate", "Mbps");
        assert_eq!(rate.value, "8.9 / 8 Mbps");
        assert!(rate.warn);
        let pipeline = tile(&history, "pipeline_ms", "Capture to encoded", "ms");
        assert_eq!(
            (pipeline.value.as_str(), pipeline.detail.as_str()),
            ("6 ms", "4.25–9")
        );
        let calm = [sample(60., &[("video_mbps", 8.7), ("target_mbps", 8.0)])];
        assert!(!tile(&calm, "video_mbps", "Video rate", "Mbps").warn);
    }

    #[test]
    fn meters_appear_only_when_the_server_reports_them() {
        assert!(meters(Some(&sample(60., &[]))).is_empty());
        let gib = 1_073_741_824.;
        let host = sample(
            60.,
            &[
                ("cpu_percent", 4.3),
                ("mem_used", 2. * gib),
                ("mem_total", 8. * gib),
            ],
        );
        let meters_now = meters(Some(&host));
        let labels: Vec<_> = meters_now.iter().map(|m| m.label.as_str()).collect();
        assert_eq!(labels, ["CPU", "MEM"]);
        assert_eq!(meters_now[0].text, "4.3%");
        assert_eq!(meters_now[1].text, "2.0 GiB / 8.0 GiB · 25%");
        let gpu = sample(
            60.,
            &[
                ("cpu_percent", 40.),
                ("gpu_percent", 12.),
                ("gpu_mem_used", gib),
                ("gpu_mem_total", 4. * gib),
                ("mem_used", gib),
                ("mem_total", 8. * gib),
            ],
        );
        let labels: Vec<_> = meters(Some(&gpu)).into_iter().map(|m| m.label).collect();
        assert_eq!(labels, ["CPU", "GPU", "MEM", "VRAM"]);
    }

    #[test]
    fn a_long_history_is_bucketed_by_maximum() {
        let history: Vec<_> = (0..240).map(|i| sample(i as f64, &[])).collect();
        let fps = series_of(&history, "fps");
        assert_eq!(fps.len(), GRAPH_POINTS);
        assert_eq!(fps[0], 1.);
        assert_eq!(fps[GRAPH_POINTS - 1], 239.);
        let graph = &graphs(&history, 60)[0];
        assert_eq!(graph.max, (239. * 1.1f32).ceil());
        assert_eq!(graph.series.len(), 1);
    }

    fn values(props: &StripProps) -> Vec<&str> {
        props.figures.iter().map(|f| f.value.as_str()).collect()
    }

    #[test]
    fn strip_reads_fps_rate_send_and_rtt() {
        let mut latest = sample(
            59.6,
            &[("video_mbps", 7.8), ("target_mbps", 8.0), ("send_ms", 12.0)],
        );
        latest.rtt_ms = Some(24.3);
        let props = strip(Some(&info()), &[latest]);
        assert_eq!(values(&props), ["60 fps", "7.8 / 8 Mbps", "12 ms", "24 ms"]);
        let captions: Vec<_> = props.figures.iter().map(|f| f.caption.as_str()).collect();
        assert_eq!(
            captions,
            ["FPS", "Video rate", "Capture to sent", "Latency"]
        );
        assert_eq!(props.marks.len(), 4);
        assert!(props.figures.iter().all(|f| !f.warn));
        let mut quick = sample(60., &[]);
        quick.rtt_ms = Some(4.25);
        assert_eq!(strip(None, &[quick]).figures[3].value, "4.3 ms");
    }

    #[test]
    fn strip_rate_warns_over_target() {
        let over = [sample(60., &[("video_mbps", 9.0), ("target_mbps", 8.0)])];
        assert!(strip(None, &over).figures[1].warn);
        let under = [sample(60., &[("video_mbps", 8.8), ("target_mbps", 8.0)])];
        assert!(!strip(None, &under).figures[1].warn);
    }

    #[test]
    fn strip_send_holds_then_idles() {
        let mut history = vec![sample(60., &[("send_ms", 12.0)])];
        for _ in 0..3 {
            history.push(sample(0., &[("encoded_fps", 0.)]));
        }
        assert_eq!(strip(None, &history).figures[2].value, "12 ms");
        for _ in 0..3 {
            history.push(sample(0., &[("encoded_fps", 0.)]));
        }
        let start = history.len() - STRIP_SAMPLES;
        assert_eq!(strip(None, &history[start..]).figures[2].value, "idle");
    }

    #[test]
    fn strip_without_server_figures_reads_dashes() {
        let props = strip(None, &[sample(30., &[])]);
        assert_eq!(values(&props), ["30 fps", "–", "–", "9 ms"]);
        assert_eq!(values(&strip(None, &[]))[0], "–");
    }

    #[test]
    fn strip_keeps_subscription_when_section_closes() {
        let mut interest = StatsInterest::new(true);
        assert!(interest.wanted());
        assert_eq!(interest.set_section(true), None);
        assert_eq!(interest.set_section(false), None);
        assert_eq!(interest.set_strip(false), Some(false));
        assert!(!interest.wanted());
    }

    #[test]
    fn subscription_starts_once_for_section_then_strip() {
        let mut interest = StatsInterest::default();
        assert_eq!(interest.set_section(true), Some(true));
        assert_eq!(interest.set_strip(true), None);
        assert_eq!(interest.set_section(false), None);
        assert_eq!(interest.set_strip(false), Some(false));
        assert_eq!(interest.set_strip(false), None);
    }

    #[test]
    fn the_report_names_rows_tiles_and_the_client() {
        let history = [sample(60., &[("encode_ms", 2.0), ("cpu_percent", 20.)])];
        let text = report(Some(&info()), &history);
        assert!(text.starts_with("encoder: x264 · H.264 4:2:0\n"));
        assert!(text.contains("fps 60 · mbps 4 · rtt_ms 9"));
        assert!(text.contains("Encode: 2 ms"));
        assert!(text.contains("cpu: 20%"));
        assert!(text.ends_with(&format!("SealSkin Native {}", env!("CARGO_PKG_VERSION"))));
    }
}
