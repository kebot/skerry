//! The account menu in the Library header (`ARCH-029`): who is signed in, what the account
//! may do, its limits with meters, where the key lives, and Sign Out. Pure presentation:
//! the Launcher fills [`AccountProps`] from SealSkin's `admin/status`.

use crate::session::KeySourceLabel;
use gpui_kit::component::{
    avatar::Avatar,
    button::{Button, ButtonVariants as _},
    popover::Popover,
    progress::Progress,
    scroll::ScrollableElement as _,
    spinner::Spinner,
    tag::Tag,
    tooltip::Tooltip,
    ActiveTheme as _, Disableable as _, IconName, Selectable as _, Sizable as _, StyledExt as _,
    Theme,
};
use gpui_kit::{
    div, prelude::*, px, Anchor, App, ClickEvent, Div, FontWeight, Hsla, SharedString, Window,
};
use std::rc::Rc;

/// The longest user name the header button shows before cutting it.
pub const NAME_MAX_CHARS: usize = 16;
/// The popover's width.
pub const MENU_WIDTH: f32 = 300.;

/// Everything the account menu shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AccountProps {
    /// The user name; empty when the account couldn't be read (the menu says so).
    pub user: String,
    pub server: String,
    pub admin: bool,
    pub groups: Vec<String>,
    /// Persistent storage, GPU, VPN, in that order.
    pub access: Vec<AccessChip>,
    /// Empty for an administrator.
    pub limits: Vec<LimitRow>,
    /// Where the key came from; `None` before it is known.
    pub key_source: Option<KeySourceLabel>,
    /// The key's non-secret id, shown only in a tooltip.
    pub key_id: String,
    /// When the key sleeps (`dormancy_text`), if a rule is set.
    pub dormancy: Option<String>,
}

impl AccountProps {
    /// The account details came back from the server.
    pub fn available(&self) -> bool {
        !self.user.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccessChip {
    pub label: SharedString,
    pub allowed: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LimitRow {
    pub label: SharedString,
    pub value: LimitValue,
    pub meter: Option<Meter>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LimitValue {
    Text(String),
    /// Storage is being measured.
    Measuring,
    /// Storage couldn't be measured (or hasn't been yet).
    Unknown,
}

/// A usage bar under a limit row.
#[derive(Clone, Debug, PartialEq)]
pub struct Meter {
    /// 0–1.
    pub fraction: f32,
    /// Shown under a full meter.
    pub caption: Option<&'static str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Normal,
    Warning,
    Danger,
}

/// The numbers the Limits section is built from; a negative limit means unlimited.
#[derive(Clone, Debug, PartialEq)]
pub struct Limits {
    pub running: usize,
    pub session_limit: i64,
    pub session_cpus: f64,
    pub session_memory_mb: i64,
    pub session_hours: f64,
    pub session_idle_minutes: f64,
    /// `None` without persistent storage.
    pub storage: Option<StorageUse>,
    /// `(used, hours, period)`; `None` when the server sends none (unlimited).
    pub allowance: Option<(f64, f64, String)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StorageUse {
    /// GB (binary) per node; negative: unlimited.
    pub limit_gb: i64,
    pub used: StorageMeasure,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StorageMeasure {
    Measuring,
    Unknown,
    Bytes(u64),
}

const GIB: f64 = 1024. * 1024. * 1024.;

pub const STORAGE_FULL: &str = "Full: new uploads are refused.";
pub const ALLOWANCE_USED_UP: &str = "Used up: new sessions can’t start.";
pub const ADMIN_NOTE: &str = "Administrators have every capability and no limits.";
pub const UNAVAILABLE_NOTE: &str =
    "Account details aren’t available right now. Choose Refresh to try again.";
pub const MEASURE_FAILED: &str = "Couldn’t measure storage. Open this menu again to retry.";

/// The header button's name: the user name cut to [`NAME_MAX_CHARS`], and whether it was
/// cut. An unknown user reads "Account".
pub fn display_name(user: &str) -> (String, bool) {
    let user = user.trim();
    if user.is_empty() {
        return ("Account".into(), false);
    }
    if user.chars().count() > NAME_MAX_CHARS {
        let cut: String = user.chars().take(NAME_MAX_CHARS).collect();
        (format!("{cut}…"), true)
    } else {
        (user.into(), false)
    }
}

/// `4`, `0.5`, `1.5`: at most one decimal, without a trailing `.0`.
fn number(value: f64) -> String {
    let rounded = (value * 10.).round() / 10.;
    if rounded.fract() == 0. {
        format!("{}", rounded as i64)
    } else {
        format!("{rounded:.1}")
    }
}

/// A limit's value, or "Unlimited" when negative.
pub fn limit_text(value: f64, format: impl FnOnce(f64) -> String) -> String {
    if value < 0. {
        "Unlimited".into()
    } else {
        format(value)
    }
}

/// Bytes in binary units with `decimals` decimals: `12.4 GB`, `512 MB`.
pub fn bytes_binary(bytes: u64, decimals: usize) -> String {
    let bytes = bytes as f64;
    if bytes >= GIB || bytes == 0. {
        format!(
            "{:.*} GB",
            if bytes == 0. { 0 } else { decimals },
            bytes / GIB
        )
    } else {
        format!("{:.*} MB", decimals, bytes / (1024. * 1024.))
    }
}

/// Memory per session from MB: `8 GB`, `512 MB`, `1.5 GB`.
fn memory_text(mb: f64) -> String {
    if mb >= 1024. {
        format!("{} GB", number(mb / 1024.))
    } else {
        format!("{} MB", number(mb))
    }
}

/// `12.4 GB of 50 GB`, or `12.4 GB used` without a limit. Used storage is in binary GB with
/// one decimal (the server enforces `limit × 1024³`, `quota.py`).
pub fn storage_text(used: u64, limit_gb: i64) -> String {
    let used = if used == 0 {
        "0 GB".to_string()
    } else {
        format!("{:.1} GB", used as f64 / GIB)
    };
    if limit_gb < 0 {
        format!("{used} used")
    } else {
        format!("{used} of {limit_gb} GB")
    }
}

/// "today", "this week", "this month" (SealSkin counts calendar periods in UTC).
pub fn period_word(period: &str) -> String {
    match period {
        "day" => "today".into(),
        "week" => "this week".into(),
        "month" => "this month".into(),
        "" => String::new(),
        other => format!("per {other}"),
    }
}

/// `12.5 of 40 h this month`.
pub fn allowance_text(used: f64, hours: f64, period: &str) -> String {
    let period = period_word(period);
    let text = format!("{used:.1} of {} h", number(hours));
    if period.is_empty() {
        text
    } else {
        format!("{text} {period}")
    }
}

/// How full a meter is, when there is a limit to measure against.
pub fn meter_fraction(used: f64, limit: f64) -> Option<f32> {
    if limit > 0. {
        Some((used / limit).clamp(0., 1.) as f32)
    } else if limit == 0. {
        Some(1.)
    } else {
        None
    }
}

/// Warning from 80 %, danger at 100 %.
pub fn meter_tone(fraction: f32) -> Tone {
    if fraction >= 1. {
        Tone::Danger
    } else if fraction >= 0.8 {
        Tone::Warning
    } else {
        Tone::Normal
    }
}

/// When the key sleeps, from `admin/status`'s settings: a rule is set when
/// `follow_sign_in` is on (the stricter rule, so it wins) or `hours >= 0`.
pub fn dormancy_text(hours: f64, follow_sign_in: bool) -> Option<String> {
    if follow_sign_in {
        return Some("Sleeps when your web sign-in ends.".into());
    }
    if hours < 0. {
        return None;
    }
    if hours == 0. {
        return Some("Sleeps as soon as you’ve signed in on the web.".into());
    }
    let duration = if hours.fract() == 0. {
        let whole = hours as i64;
        format!("{whole} hour{}", if whole == 1 { "" } else { "s" })
    } else {
        let minutes = (hours * 60.).round() as i64;
        format!("{minutes} minute{}", if minutes == 1 { "" } else { "s" })
    };
    Some(format!("Sleeps {duration} after your last web sign-in."))
}

/// The Key section's first line.
pub fn key_source_text(source: KeySourceLabel) -> &'static str {
    match source {
        KeySourceLabel::SavedFile => "Saved in ~/.config/skerry/config.toml",
        KeySourceLabel::Environment => "From the SEALSKIN_APPLICATION_KEY environment variable",
        KeySourceLabel::DotEnv => "From SEALSKIN_APPLICATION_KEY in .env",
        KeySourceLabel::ThisRunOnly => "Not saved: used for this run only",
    }
}

/// The Limits rows, in the web profile's order (`client/src/ui/app.js` `renderProfile`).
pub fn limit_rows(limits: &Limits) -> Vec<LimitRow> {
    let text = |label: &'static str, value: String| LimitRow {
        label: label.into(),
        value: LimitValue::Text(value),
        meter: None,
    };
    let sessions = if limits.session_limit < 0 {
        format!("{} running · Unlimited", limits.running)
    } else {
        format!("{} running of {}", limits.running, limits.session_limit)
    };
    let mut rows = vec![
        text("Sessions", sessions),
        text("CPUs per session", limit_text(limits.session_cpus, number)),
        text(
            "Memory per session",
            limit_text(limits.session_memory_mb as f64, memory_text),
        ),
        text(
            "Session length",
            limit_text(limits.session_hours, |h| format!("{} h", number(h))),
        ),
        text(
            "Time with nobody connected",
            limit_text(limits.session_idle_minutes, |m| {
                format!("{} min", number(m))
            }),
        ),
    ];
    if let Some(storage) = &limits.storage {
        let (value, meter) = match storage.used {
            StorageMeasure::Measuring => (LimitValue::Measuring, None),
            StorageMeasure::Unknown => (LimitValue::Unknown, None),
            StorageMeasure::Bytes(used) => (
                LimitValue::Text(storage_text(used, storage.limit_gb)),
                meter_fraction(used as f64, storage.limit_gb as f64 * GIB).map(|fraction| Meter {
                    fraction,
                    caption: (fraction >= 1.).then_some(STORAGE_FULL),
                }),
            ),
        };
        rows.push(LimitRow {
            label: "Storage".into(),
            value,
            meter,
        });
    }
    if let Some((used, hours, period)) = &limits.allowance {
        rows.push(LimitRow {
            label: "Allowance".into(),
            value: LimitValue::Text(allowance_text(*used, *hours, period)),
            meter: meter_fraction(*used, *hours)
                .filter(|_| *hours > 0.)
                .map(|fraction| Meter {
                    fraction,
                    caption: (fraction >= 1.).then_some(ALLOWANCE_USED_UP),
                }),
        });
    }
    rows
}

/// Called with the popover's new open state.
pub type OpenHandler = Rc<dyn Fn(bool, &mut Window, &mut App)>;
/// A menu action.
pub type ActionHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// What the account menu does.
#[derive(Clone)]
pub struct AccountHandlers {
    /// The popover opened (`true`) or closed.
    pub on_open_change: OpenHandler,
    pub on_open_web: ActionHandler,
    /// Asks to sign out; the caller shows the confirmation.
    pub on_sign_out: ActionHandler,
}

/// The header's account button and its popover.
pub fn render_menu(
    props: &AccountProps,
    open: bool,
    enabled: bool,
    handlers: AccountHandlers,
) -> impl IntoElement {
    let (name, cut) = display_name(&props.user);
    let tooltip = if cut {
        format!("Account · {}", props.user.trim())
    } else {
        "Account".into()
    };
    let trigger = Button::new("account")
        .ghost()
        .xsmall()
        .icon(IconName::CircleUser)
        .label(name)
        .dropdown_caret(true)
        .tooltip(tooltip)
        .disabled(!enabled)
        .selected(open);
    let on_open_change = handlers.on_open_change.clone();
    let props = props.clone();
    Popover::new("account-menu")
        .anchor(Anchor::TopRight)
        .overlay_closable(true)
        .open(open && enabled)
        .on_open_change(move |open, window, cx| on_open_change(*open, window, cx))
        .trigger(trigger)
        .content(move |_, window, cx| {
            let theme = cx.theme().clone();
            let max_height = window.viewport_size().height - px(96.);
            panel(&theme, &props, &handlers, max_height)
        })
}

fn section_label(theme: &Theme, label: &'static str) -> Div {
    div()
        .text_xs()
        .text_color(theme.muted_foreground)
        .child(label)
}

fn hairline(theme: &Theme) -> Div {
    div().h(px(1.)).w_full().bg(theme.border)
}

/// The popover's body and footer.
pub fn panel(
    theme: &Theme,
    props: &AccountProps,
    handlers: &AccountHandlers,
    max_height: gpui_kit::Pixels,
) -> Div {
    let available = props.available();
    let avatar = if available {
        Avatar::new().name(props.user.clone()).small()
    } else {
        Avatar::new().placeholder(IconName::CircleUser).small()
    };
    let identity = div().h_flex().items_center().gap_2p5().child(avatar).child(
        div()
            .v_flex()
            .min_w_0()
            .child(
                div()
                    .h_flex()
                    .items_center()
                    .gap_1p5()
                    .min_w_0()
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(if available {
                                props.user.clone()
                            } else {
                                "Account".into()
                            }),
                    )
                    .when(props.admin, |row| {
                        row.child(Tag::secondary().xsmall().child("Admin"))
                    }),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(props.server.clone()),
            ),
    );

    let mut body = div().v_flex().gap_3().child(identity);
    if available && (!props.admin || !props.groups.is_empty()) {
        let groups = if props.groups.is_empty() {
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child("No group: the server’s defaults apply.")
        } else {
            div().h_flex().flex_wrap().gap_1().children(
                props
                    .groups
                    .iter()
                    .map(|group| Tag::secondary().xsmall().child(group.clone())),
            )
        };
        body = body.child(
            div()
                .v_flex()
                .gap_1p5()
                .child(section_label(theme, "Groups"))
                .child(groups),
        );
    }
    if !available {
        body = body.child(hairline(theme)).child(
            div()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(UNAVAILABLE_NOTE),
        );
    } else if props.admin {
        body = body.child(hairline(theme)).child(
            div()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(ADMIN_NOTE),
        );
    } else {
        body = body
            .child(hairline(theme))
            .child(
                div()
                    .v_flex()
                    .gap_1p5()
                    .child(section_label(theme, "Access"))
                    .child(
                        div()
                            .h_flex()
                            .flex_wrap()
                            .gap_1p5()
                            .children(props.access.iter().map(|chip| access_chip(theme, chip))),
                    ),
            )
            .child(hairline(theme))
            .child(
                div()
                    .v_flex()
                    .gap_1p5()
                    .child(section_label(theme, "Limits"))
                    .children(
                        props
                            .limits
                            .iter()
                            .enumerate()
                            .map(|(index, row)| limit_row(theme, index, row)),
                    ),
            );
    }
    if let Some(source) = props.key_source {
        let key_id = props.key_id.clone();
        body = body.child(hairline(theme)).child(
            div()
                .v_flex()
                .gap_1()
                .child(section_label(theme, "Key"))
                .child(
                    div()
                        .id("account-key-source")
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(key_source_text(source))
                        .when(!key_id.is_empty(), |line| {
                            line.tooltip(move |window, cx| {
                                Tooltip::new(format!("Key ID {key_id}")).build(window, cx)
                            })
                        }),
                )
                .when_some(props.dormancy.clone(), |section, dormancy| {
                    section.child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(dormancy),
                    )
                }),
        );
    }

    let on_open_web = handlers.on_open_web.clone();
    let close_web = handlers.on_open_change.clone();
    let on_sign_out = handlers.on_sign_out.clone();
    let close_sign_out = handlers.on_open_change.clone();
    let footer = div()
        .v_flex()
        .gap_0p5()
        .child(
            Button::new("account-open-web")
                .ghost()
                .small()
                .w_full()
                .justify_start()
                .icon(IconName::ExternalLink)
                .label("Open SealSkin in Browser")
                .on_click(move |event, window, cx| {
                    close_web(false, window, cx);
                    on_open_web(event, window, cx);
                }),
        )
        .child(
            Button::new("account-sign-out")
                .ghost()
                .small()
                .w_full()
                .justify_start()
                .text_color(theme.danger)
                .icon(gpui_kit::assets::IconName::LogOut)
                .label("Sign Out…")
                // The popover closes first, then the confirmation opens (Task A's order).
                .on_click(move |event, window, cx| {
                    close_sign_out(false, window, cx);
                    on_sign_out(event, window, cx);
                }),
        );

    div()
        .w(px(MENU_WIDTH))
        .v_flex()
        .gap_3()
        .child(
            div()
                .id("account-menu-body")
                .max_h(max_height)
                .overflow_y_scrollbar()
                .child(body),
        )
        .child(hairline(theme))
        .child(footer)
}

fn access_chip(theme: &Theme, chip: &AccessChip) -> impl IntoElement {
    let label = chip.label.clone();
    let pill = div()
        .id(SharedString::from(format!("access-{}", chip.label)))
        .h_flex()
        .items_center()
        .gap_1()
        .px_2()
        .py_0p5()
        .rounded_md()
        .text_xs();
    if chip.allowed {
        pill.bg(theme.secondary)
            .text_color(theme.foreground)
            .child(div().text_color(theme.success).child(IconName::Check))
            .child(label)
    } else {
        pill.border_1()
            .border_color(theme.border)
            .text_color(theme.muted_foreground)
            .child(IconName::Minus)
            .child(label)
            .tooltip(|window, cx| Tooltip::new("Not allowed for this account").build(window, cx))
    }
}

fn tone_color(theme: &Theme, tone: Tone) -> Option<Hsla> {
    match tone {
        Tone::Normal => None,
        Tone::Warning => Some(theme.warning),
        Tone::Danger => Some(theme.danger),
    }
}

fn limit_row(theme: &Theme, index: usize, row: &LimitRow) -> impl IntoElement {
    let tone = row
        .meter
        .as_ref()
        .map_or(Tone::Normal, |meter| meter_tone(meter.fraction));
    let value: gpui_kit::AnyElement = match &row.value {
        LimitValue::Text(text) => div()
            .text_color(if tone == Tone::Danger {
                theme.danger
            } else {
                theme.foreground
            })
            .child(text.clone())
            .into_any_element(),
        LimitValue::Measuring => div()
            .h_flex()
            .items_center()
            .gap_1p5()
            .text_color(theme.muted_foreground)
            .child(Spinner::new().xsmall())
            .child("Measuring…")
            .into_any_element(),
        LimitValue::Unknown => div()
            .id(SharedString::from(format!("limit-unknown-{index}")))
            .text_color(theme.muted_foreground)
            .child("—")
            .tooltip(|window, cx| Tooltip::new(MEASURE_FAILED).build(window, cx))
            .into_any_element(),
    };
    div()
        .v_flex()
        .child(
            div()
                .h_flex()
                .justify_between()
                .gap_3()
                .text_sm()
                .child(
                    div()
                        .flex_none()
                        .text_color(theme.muted_foreground)
                        .child(row.label.clone()),
                )
                .child(div().min_w_0().truncate().child(value)),
        )
        .when_some(row.meter.clone(), |view, meter| {
            let mut progress = Progress::new(SharedString::from(format!("limit-meter-{index}")))
                .xsmall()
                .value(meter.fraction * 100.)
                .accessibility_label(format!("{} {}", row.label, value_text(&row.value)));
            if let Some(color) = tone_color(theme, tone) {
                progress = progress.color(color);
            }
            view.child(div().mt_1().child(progress))
                .when_some(meter.caption, |view, caption| {
                    view.child(
                        div()
                            .mt_1()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(caption),
                    )
                })
        })
}

fn value_text(value: &LimitValue) -> String {
    match value {
        LimitValue::Text(text) => text.clone(),
        LimitValue::Measuring => "Measuring…".into(),
        LimitValue::Unknown => "—".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_name_cuts_long_names_and_falls_back_to_account() {
        assert_eq!(display_name("kebot"), ("kebot".into(), false));
        assert_eq!(display_name(""), ("Account".into(), false));
        assert_eq!(
            display_name("a-very-long-user-name-indeed"),
            ("a-very-long-user…".into(), true)
        );
    }

    #[test]
    fn limits_read_unlimited_when_negative() {
        assert_eq!(limit_text(-1., number), "Unlimited");
        assert_eq!(limit_text(4., number), "4");
        assert_eq!(limit_text(0.5, number), "0.5");
        assert_eq!(memory_text(8192.), "8 GB");
        assert_eq!(memory_text(512.), "512 MB");
        assert_eq!(memory_text(1536.), "1.5 GB");
    }

    #[test]
    fn storage_uses_binary_gigabytes() {
        let used = (12.4 * GIB) as u64;
        assert_eq!(storage_text(used, 50), "12.4 GB of 50 GB");
        assert_eq!(storage_text(0, 50), "0 GB of 50 GB");
        assert_eq!(storage_text(used, -1), "12.4 GB used");
        assert_eq!(bytes_binary(512 * 1024 * 1024, 0), "512 MB");
        // Half of a 50 GiB limit is half full, not 53.7 GB's worth.
        let half = meter_fraction(25. * GIB, 50. * GIB).unwrap();
        assert!((half - 0.5).abs() < 1e-6);
        assert_eq!(meter_fraction(10., -1.), None);
        // A zero limit is full (the server refuses at used >= 0).
        assert_eq!(meter_fraction(0., 0.), Some(1.));
    }

    #[test]
    fn meter_turns_warning_at_80_and_danger_at_100() {
        assert_eq!(meter_tone(0.79), Tone::Normal);
        assert_eq!(meter_tone(0.8), Tone::Warning);
        assert_eq!(meter_tone(0.99), Tone::Warning);
        assert_eq!(meter_tone(1.0), Tone::Danger);
    }

    #[test]
    fn allowance_text_names_the_period() {
        assert_eq!(
            allowance_text(12.5, 40., "month"),
            "12.5 of 40 h this month"
        );
        assert_eq!(allowance_text(40., 40., "week"), "40.0 of 40 h this week");
        assert_eq!(allowance_text(1., 2.5, "day"), "1.0 of 2.5 h today");
    }

    #[test]
    fn dormancy_prefers_follow_sign_in() {
        assert_eq!(
            dormancy_text(24., true).as_deref(),
            Some("Sleeps when your web sign-in ends.")
        );
        assert_eq!(
            dormancy_text(24., false).as_deref(),
            Some("Sleeps 24 hours after your last web sign-in.")
        );
        assert_eq!(
            dormancy_text(1., false).as_deref(),
            Some("Sleeps 1 hour after your last web sign-in.")
        );
        assert_eq!(
            dormancy_text(1.5, false).as_deref(),
            Some("Sleeps 90 minutes after your last web sign-in.")
        );
        // 0 is a rule too: the key sleeps right after each wake.
        assert!(dormancy_text(0., false).is_some());
        assert_eq!(dormancy_text(-1., false), None);
    }

    fn limits() -> Limits {
        Limits {
            running: 1,
            session_limit: 2,
            session_cpus: 4.,
            session_memory_mb: 8192,
            session_hours: 8.,
            session_idle_minutes: 30.,
            storage: Some(StorageUse {
                limit_gb: 50,
                used: StorageMeasure::Bytes((12.4 * GIB) as u64),
            }),
            allowance: Some((40., 40., "month".into())),
        }
    }

    #[test]
    fn limit_rows_follow_the_web_order_with_meters() {
        let rows = limit_rows(&limits());
        let labels: Vec<&str> = rows.iter().map(|row| row.label.as_ref()).collect();
        assert_eq!(
            labels,
            [
                "Sessions",
                "CPUs per session",
                "Memory per session",
                "Session length",
                "Time with nobody connected",
                "Storage",
                "Allowance"
            ]
        );
        assert_eq!(rows[0].value, LimitValue::Text("1 running of 2".into()));
        assert_eq!(rows[5].meter.as_ref().unwrap().caption, None);
        let allowance = rows[6].meter.as_ref().unwrap();
        assert_eq!(allowance.caption, Some(ALLOWANCE_USED_UP));
    }

    #[test]
    fn sessions_unlimited_shows_running_count() {
        let rows = limit_rows(&Limits {
            session_limit: -1,
            running: 2,
            storage: None,
            allowance: None,
            ..limits()
        });
        assert_eq!(
            rows[0].value,
            LimitValue::Text("2 running · Unlimited".into())
        );
        assert_eq!(rows.len(), 5);
        let measuring = limit_rows(&Limits {
            storage: Some(StorageUse {
                limit_gb: 50,
                used: StorageMeasure::Measuring,
            }),
            ..limits()
        });
        assert_eq!(measuring[5].value, LimitValue::Measuring);
        assert!(measuring[5].meter.is_none());
    }
}
