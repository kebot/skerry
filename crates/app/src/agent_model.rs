//! Pure logic for the Agent marker (a violet ✦): which Tabs and chips get it
//! and how it pulses. No GPUI, no I/O. One marker in v1: a subtle pulse while
//! the Agent is acting now, static once it touched a Tab earlier.

use std::time::{Duration, Instant};

use rb_cdp::agent_proxy::{AgentState, ACTING_WINDOW};

/// Period of one pulse (the mock's `pulse 1.6s`).
pub const PULSE_PERIOD: Duration = Duration::from_millis(1600);
/// Lowest opacity of the pulse (the mock: `50% { opacity: .35 }`).
pub const PULSE_MIN: f32 = 0.35;

/// How a Tab is marked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Marker {
    None,
    /// The Agent touched this Tab earlier: static ✦.
    Touched,
    /// The Agent acted on this Tab within [`ACTING_WINDOW`]: pulsing ✦.
    Acting,
}

impl Marker {
    pub fn shown(self) -> bool {
        self != Marker::None
    }
}

/// The marker for `target_id`, or none without a state (proxy off).
pub fn marker(state: Option<&AgentState>, target_id: Option<&str>, now: Instant) -> Marker {
    let (Some(s), Some(t)) = (state, target_id) else {
        return Marker::None;
    };
    if s.is_acting(t, ACTING_WINDOW, now) {
        Marker::Acting
    } else if s.touched(t) {
        Marker::Touched
    } else {
        Marker::None
    }
}

/// The chip shows ✦ while at least one Agent client is attached.
pub fn chip_marked(state: Option<&AgentState>) -> bool {
    state.is_some_and(|s| s.clients >= 1)
}

/// Opacity of a pulsing marker `elapsed` into the animation: a triangle wave
/// between 1.0 and [`PULSE_MIN`]. A static marker is always 1.0.
pub fn opacity(marker: Marker, elapsed: Duration) -> f32 {
    if marker != Marker::Acting {
        return 1.0;
    }
    let period = PULSE_PERIOD.as_secs_f32();
    let phase = (elapsed.as_secs_f32() % period) / period; // 0..1
    let tri = (phase * 2.0 - 1.0).abs(); // 1 -> 0 -> 1
    PULSE_MIN + (1.0 - PULSE_MIN) * tri
}

/// How long until the marker of `target_id` stops pulsing (so the UI can
/// repaint once more), or `None` if it is not acting.
pub fn acting_left(state: &AgentState, target_id: &str, now: Instant) -> Option<Duration> {
    let a = state.targets.get(target_id)?;
    let age = now.saturating_duration_since(a.last_acted_at);
    ACTING_WINDOW.checked_sub(age).filter(|d| !d.is_zero())
}

/// Hints shown under the endpoint in the chip popover.
pub fn usage_hint(endpoint: &str) -> String {
    format!(
        "Playwright: connect_over_cdp(\"{endpoint}\")\nPlaywright MCP: --cdp-endpoint {endpoint}"
    )
}

/// What the chip popover says about the Agent endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentLine {
    /// Proxy not enabled for this Connection.
    Off,
    /// Enabled, waiting for the Connection (or the bind).
    Waiting,
    /// Running at this endpoint with this many clients attached.
    On { endpoint: String, clients: usize },
    /// Could not start.
    Error(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use rb_cdp::agent_proxy::TargetActivity;

    fn state(now: Instant, clients: usize, acts: &[(&str, Duration)]) -> AgentState {
        let mut s = AgentState {
            clients,
            ..Default::default()
        };
        for (t, ago) in acts {
            s.targets.insert(
                t.to_string(),
                TargetActivity {
                    last_acted_at: now - *ago,
                    last_method: "Page.navigate".into(),
                    emulation_override: false,
                },
            );
        }
        s
    }

    #[test]
    fn acting_touched_none() {
        let now = Instant::now();
        let s = state(
            now,
            1,
            &[
                ("A", Duration::from_millis(300)),
                ("B", Duration::from_secs(30)),
            ],
        );
        assert_eq!(marker(Some(&s), Some("A"), now), Marker::Acting);
        assert_eq!(marker(Some(&s), Some("B"), now), Marker::Touched);
        assert_eq!(marker(Some(&s), Some("C"), now), Marker::None);
        assert_eq!(marker(Some(&s), None, now), Marker::None);
        assert_eq!(marker(None, Some("A"), now), Marker::None);
        // Acting turns into touched as time passes.
        assert_eq!(
            marker(
                Some(&s),
                Some("A"),
                now + ACTING_WINDOW + Duration::from_millis(1)
            ),
            Marker::Touched
        );
    }

    #[test]
    fn chip_needs_a_client_but_touched_marks_survive_detach() {
        let now = Instant::now();
        let mut s = state(now, 1, &[("A", Duration::from_secs(10))]);
        assert!(chip_marked(Some(&s)));
        s.clients = 0;
        assert!(!chip_marked(Some(&s)));
        assert_eq!(marker(Some(&s), Some("A"), now), Marker::Touched);
        assert!(!chip_marked(None));
    }

    #[test]
    fn pulse_is_a_triangle_between_min_and_one() {
        let m = Marker::Acting;
        assert_eq!(opacity(Marker::Touched, Duration::from_millis(400)), 1.0);
        assert!((opacity(m, Duration::ZERO) - 1.0).abs() < 1e-4);
        assert!((opacity(m, PULSE_PERIOD / 2) - PULSE_MIN).abs() < 1e-4);
        assert!((opacity(m, PULSE_PERIOD) - 1.0).abs() < 1e-4);
        for ms in (0..4000).step_by(37) {
            let o = opacity(m, Duration::from_millis(ms));
            assert!((PULSE_MIN - 1e-4..=1.0 + 1e-4).contains(&o), "{o}");
        }
    }

    #[test]
    fn acting_left_counts_down() {
        let now = Instant::now();
        let s = state(
            now,
            1,
            &[
                ("A", Duration::from_millis(500)),
                ("B", Duration::from_secs(9)),
            ],
        );
        let left = acting_left(&s, "A", now).unwrap();
        assert!(left <= ACTING_WINDOW - Duration::from_millis(500));
        assert!(left > Duration::from_millis(1400));
        assert_eq!(acting_left(&s, "B", now), None);
        assert_eq!(acting_left(&s, "Z", now), None);
    }

    #[test]
    fn hint_names_both_clients() {
        let h = usage_hint("http://127.0.0.1:1/x");
        assert!(h.contains("connect_over_cdp(\"http://127.0.0.1:1/x\")"));
        assert!(h.contains("--cdp-endpoint http://127.0.0.1:1/x"));
    }
}
