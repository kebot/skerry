use crate::library::OpeningKind;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    label::Label,
    select::{SearchableVec, Select, SelectItem, SelectState},
    Disableable as _, Icon, IconName, IndexPath, Sizable as _, StyledExt as _, Theme,
};
use gpui_kit::{
    div, img, prelude::*, px, rgb, rgba, App, Div, Entity, FontWeight, ObjectFit, RenderImage,
    SharedString, Window,
};
use std::rc::Rc;
use std::sync::Arc;

pub type LaunchDialogHandler = Rc<dyn Fn(LaunchDialogAction, &mut Window, &mut App)>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchOption {
    pub id: String,
    pub label: String,
}

impl SelectItem for LaunchOption {
    type Value = String;

    fn title(&self) -> SharedString {
        self.label.clone().into()
    }

    fn value(&self) -> &Self::Value {
        &self.id
    }
}

#[derive(Clone, Debug)]
pub struct LaunchDialogProps {
    pub app_id: String,
    pub app_name: String,
    pub app_icon: Option<Arc<RenderImage>>,
    pub favorite: bool,
    pub hidden: bool,
    pub storage_options: Vec<LaunchOption>,
    pub selected_storage: String,
    /// "None" first, then the GPUs that fit the app; empty hides the field.
    pub gpu_options: Vec<LaunchOption>,
    pub selected_gpu: String,
    /// "No VPN" first, then the VPN profiles open to the user; empty hides the field.
    pub vpn_options: Vec<LaunchOption>,
    pub selected_vpn: String,
    /// The chosen VPN profile's endpoints; shown only when there are two or more to pick from.
    pub vpn_endpoint_options: Vec<LaunchOption>,
    pub selected_vpn_endpoint: String,
    /// Every locale the server offers; the field is searchable.
    pub language_options: Vec<LaunchOption>,
    pub selected_language: String,
    pub collaborative: bool,
    pub wayland: bool,
    pub busy: bool,
    pub show_copy_link: bool,
    /// Shows Favorite / Hide, saved per server (`ARCH-027`). Off only in the preview's
    /// Minimal story.
    pub show_organize: bool,
    /// The deep link was just copied; the Copy button confirms it in place.
    pub link_copied: bool,
    /// A running session these options would duplicate: the footer offers to open it.
    pub running_match: Option<RunningMatch>,
    /// The Launch & Open variant: the link or file this launch opens (`ARCH-028`). Copy link
    /// and the running-session footer hide, since neither carries it.
    pub opening: Option<OpeningLabel>,
}

/// What a Launch & Open dialog opens, as its context strip shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpeningLabel {
    pub kind: OpeningKind,
    /// The link's label (the real host first) or the file name.
    pub label: String,
    /// "PDF · 2.4 MB" (files only).
    pub detail: Option<String>,
    /// The full address or path.
    pub tooltip: Option<String>,
}

/// The running session the dialog's storage and room flag match (`ARCH-027`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunningMatch {
    /// Lowercase, as it follows a "·": "started 12m ago".
    pub started_label: Option<String>,
}

/// The items behind one Launch Dialog dropdown; searchable so long lists (languages) filter as you type.
pub type LaunchOptions = SearchableVec<LaunchOption>;
pub type LaunchSelect = Entity<SelectState<LaunchOptions>>;

/// Live `gpui-kit` `SelectState` entities backing the Launch Dialog's native dropdown popovers.
#[derive(Clone)]
pub struct LaunchDialogSelects {
    pub storage: Option<LaunchSelect>,
    pub gpu: Option<LaunchSelect>,
    pub vpn: Option<LaunchSelect>,
    pub vpn_endpoint: Option<LaunchSelect>,
    pub language: Option<LaunchSelect>,
}

impl LaunchDialogSelects {
    pub fn from_props(props: &LaunchDialogProps, window: &mut Window, cx: &mut App) -> Self {
        Self {
            storage: build_select(
                &props.storage_options,
                &props.selected_storage,
                window,
                cx,
                false,
            ),
            gpu: build_select(&props.gpu_options, &props.selected_gpu, window, cx, false),
            vpn: build_select(&props.vpn_options, &props.selected_vpn, window, cx, false),
            vpn_endpoint: Self::build_vpn_endpoint(props, window, cx),
            language: build_select(
                &props.language_options,
                &props.selected_language,
                window,
                cx,
                true,
            ),
        }
    }

    /// The endpoint field, which only shows when the chosen VPN has more than one endpoint.
    pub fn build_vpn_endpoint(
        props: &LaunchDialogProps,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<LaunchSelect> {
        if props.vpn_endpoint_options.len() < 2 {
            return None;
        }
        build_select(
            &props.vpn_endpoint_options,
            &props.selected_vpn_endpoint,
            window,
            cx,
            false,
        )
    }
}

fn build_select(
    options: &[LaunchOption],
    selected_id: &str,
    window: &mut Window,
    cx: &mut App,
    searchable: bool,
) -> Option<LaunchSelect> {
    if options.is_empty() {
        return None;
    }
    let selected_index = options
        .iter()
        .position(|opt| opt.id == selected_id)
        .or(Some(0))
        .map(IndexPath::new);
    let items = SearchableVec::new(options.to_vec());
    Some(cx.new(|cx| SelectState::new(items, selected_index, window, cx).searchable(searchable)))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaunchDialogAction {
    Close,
    ToggleFavorite,
    ToggleHidden,
    SetCollaborative(bool),
    SetWayland(bool),
    CopyLink,
    /// Launch (or, with a running match, Launch New).
    Launch,
    /// Open the matching running session instead of launching.
    OpenRunning,
}

/// Renders the Launch Application dialog centered over a dimmed modal backdrop.
pub fn render_modal(
    theme: &Theme,
    props: &LaunchDialogProps,
    selects: &LaunchDialogSelects,
    on_action: LaunchDialogHandler,
) -> gpui_kit::Stateful<Div> {
    let close_backdrop = Rc::clone(&on_action);
    div()
        .id("launch-dialog-backdrop")
        .absolute()
        .inset_0()
        .bg(rgba(0x00000099))
        .flex()
        .items_center()
        .justify_center()
        .p_6()
        // Block hits to the Library underneath; otherwise a backdrop click also lands on
        // whatever sits below it (e.g. Disconnect Server).
        .occlude()
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            close_backdrop(LaunchDialogAction::Close, window, cx)
        })
        .child(render_card(theme, props, selects, on_action))
}

/// Renders the standalone Launch Application dialog card.
pub fn render_card(
    theme: &Theme,
    props: &LaunchDialogProps,
    selects: &LaunchDialogSelects,
    on_action: LaunchDialogHandler,
) -> gpui_kit::Stateful<Div> {
    let enabled = !props.busy;

    let icon_view = if let Some(icon) = props.app_icon.clone() {
        div()
            .size(px(38.))
            .rounded_lg()
            .overflow_hidden()
            .child(img(icon).size_full().object_fit(ObjectFit::Contain))
    } else {
        let initial = props
            .app_name
            .chars()
            .next()
            .map(|c| c.to_ascii_uppercase().to_string())
            .unwrap_or_else(|| "?".into());
        div()
            .size(px(38.))
            .rounded_lg()
            .bg(rgb(0x5b6ef5))
            .text_color(rgb(0xffffff))
            .flex()
            .items_center()
            .justify_center()
            .font_weight(FontWeight::BOLD)
            .child(initial)
    };

    let on_fav = Rc::clone(&on_action);
    let on_hide = Rc::clone(&on_action);
    let on_close = Rc::clone(&on_action);
    let header = div()
        .px_5()
        .py_4()
        .border_b_1()
        .border_color(theme.border)
        .h_flex()
        .items_center()
        .justify_between()
        .gap_3()
        .child(
            div()
                .h_flex()
                .items_center()
                .gap_3()
                .min_w_0()
                .child(icon_view)
                .child(
                    div()
                        .v_flex()
                        .gap_0p5()
                        .min_w_0()
                        .child(
                            div()
                                .text_lg()
                                .font_weight(FontWeight::SEMIBOLD)
                                .truncate()
                                .child(props.app_name.clone()),
                        )
                        .children(subtitle(theme, props)),
                ),
        )
        .child(
            div()
                .h_flex()
                .items_center()
                .gap_2()
                .flex_none()
                .when(props.show_organize, |row| {
                    // Toggle buttons: the icon stays, the fill (or the gold star) shows
                    // the state.
                    row.child(
                        Button::new("launch-favorite")
                            .small()
                            .outline()
                            .icon(if props.favorite {
                                Icon::new(IconName::StarFill).text_color(theme.warning)
                            } else {
                                Icon::new(IconName::Star)
                            })
                            .tooltip(if props.favorite {
                                "Remove from Favorites"
                            } else {
                                "Add to Favorites"
                            })
                            .disabled(!enabled)
                            .on_click(move |_, window, cx| {
                                on_fav(LaunchDialogAction::ToggleFavorite, window, cx)
                            }),
                    )
                })
                .when(props.show_organize, |row| {
                    row.child({
                        let mut btn = Button::new("launch-hide")
                            .small()
                            .icon(IconName::EyeOff)
                            .tooltip(if props.hidden {
                                "Show in Library"
                            } else {
                                "Hide from Library"
                            })
                            .disabled(!enabled)
                            .on_click(move |_, window, cx| {
                                on_hide(LaunchDialogAction::ToggleHidden, window, cx)
                            });
                        if props.hidden {
                            btn = btn.primary();
                        } else {
                            btn = btn.outline();
                        }
                        btn
                    })
                })
                .child(
                    Button::new("launch-close")
                        .outline()
                        .small()
                        .icon(IconName::Close)
                        .tooltip("Close")
                        .on_click(move |_, window, cx| {
                            on_close(LaunchDialogAction::Close, window, cx)
                        }),
                ),
        );

    let on_collab = Rc::clone(&on_action);
    let on_wayland = Rc::clone(&on_action);
    let body = div()
        .p_5()
        .v_flex()
        .gap_4()
        .when_some(props.opening.as_ref(), |body, opening| {
            body.child(opening_strip(theme, opening))
        })
        .when_some(selects.storage.as_ref(), |body, state| {
            body.child(select_field(theme, "Storage", state, enabled))
        })
        .when_some(selects.gpu.as_ref(), |body, state| {
            body.child(select_field(theme, "GPU", state, enabled))
        })
        .when_some(selects.vpn.as_ref(), |body, state| {
            body.child(select_field(theme, "VPN", state, enabled))
        })
        .when_some(selects.vpn_endpoint.as_ref(), |body, state| {
            body.child(select_field(theme, "VPN endpoint", state, enabled))
        })
        .when_some(selects.language.as_ref(), |body, state| {
            body.child(
                div()
                    .v_flex()
                    .gap_1p5()
                    .child(field_label(theme, "Language"))
                    .child(
                        Select::new(state)
                            .search_placeholder("Search languages")
                            .disabled(!enabled),
                    ),
            )
        })
        .child(
            div()
                .pt_1()
                .v_flex()
                .gap_3()
                .child(
                    Checkbox::new("launch-collaborative")
                        .label("Collaborative Session")
                        .checked(props.collaborative)
                        .disabled(!enabled)
                        .on_click(move |checked, window, cx| {
                            on_collab(LaunchDialogAction::SetCollaborative(*checked), window, cx)
                        }),
                )
                .child(
                    Checkbox::new("launch-wayland")
                        .label("Wayland Mode")
                        .checked(props.wayland)
                        .disabled(!enabled)
                        .on_click(move |checked, window, cx| {
                            on_wayland(LaunchDialogAction::SetWayland(*checked), window, cx)
                        }),
                ),
        );

    let on_copy = Rc::clone(&on_action);
    let on_launch = Rc::clone(&on_action);
    let on_open = Rc::clone(&on_action);
    let on_launch_new = Rc::clone(&on_action);
    let opening = props.opening.is_some();
    let running = props.running_match.is_some() && !opening;
    let show_copy_link = props.show_copy_link && !opening;
    let launch_label = match (props.busy, opening) {
        (true, _) => "Launching…",
        (false, true) => "Launch & Open",
        (false, false) => "Launch",
    };
    let copy_label = if props.link_copied {
        "Copied"
    } else {
        "Copy link"
    };
    let footer = div()
        .px_5()
        .py_4()
        .border_t_1()
        .border_color(theme.border)
        .h_flex()
        .items_center()
        .gap_3()
        .when(show_copy_link, |footer| {
            let copy = Button::new("launch-copy-link")
                .outline()
                .icon(if props.link_copied {
                    IconName::Check
                } else {
                    IconName::Copy
                })
                .disabled(!enabled)
                .on_click(move |_, window, cx| on_copy(LaunchDialogAction::CopyLink, window, cx));
            // Three labelled buttons don't fit 420 pt; Copy link gives up its label.
            footer.child(if running {
                copy.tooltip(copy_label)
            } else {
                copy.label(copy_label)
            })
        })
        .when(running, |footer| {
            footer
                .child(
                    Button::new("launch-new")
                        .outline()
                        .icon(IconName::Play)
                        .label("Launch New")
                        .disabled(!enabled)
                        .on_click(move |_, window, cx| {
                            on_launch_new(LaunchDialogAction::Launch, window, cx)
                        }),
                )
                .child(
                    div().flex_1().child(
                        Button::new("launch-open-running")
                            .primary()
                            .w_full()
                            .icon(IconName::ExternalLink)
                            .label("Open Running Session")
                            .disabled(!enabled)
                            .on_click(move |_, window, cx| {
                                on_open(LaunchDialogAction::OpenRunning, window, cx)
                            }),
                    ),
                )
        })
        .when(!running, |footer| {
            footer.child(
                div().flex_1().child(
                    Button::new("launch-submit")
                        .primary()
                        .w_full()
                        .icon(IconName::Play)
                        .label(launch_label)
                        .disabled(!enabled)
                        .on_click(move |_, window, cx| {
                            on_launch(LaunchDialogAction::Launch, window, cx)
                        }),
                ),
            )
        });

    div()
        .id("launch-dialog-card")
        .w(px(460.))
        .max_w_full()
        .rounded_xl()
        .border_1()
        .border_color(theme.border)
        .bg(theme.popover)
        .text_color(theme.foreground)
        .v_flex()
        .on_click(|_, _, cx| cx.stop_propagation())
        .child(header)
        .child(body)
        .child(footer)
}

/// "Opens “report.pdf”" or "Opens wiki.lan/page", above the fields.
fn opening_strip(theme: &Theme, opening: &OpeningLabel) -> gpui_kit::Stateful<Div> {
    let icon = match opening.kind {
        OpeningKind::Link => Icon::new(gpui_kit::assets::IconName::Link),
        OpeningKind::File => Icon::new(IconName::FileText),
    };
    let name = match opening.kind {
        OpeningKind::File => format!("“{}”", opening.label),
        OpeningKind::Link => opening.label.clone(),
    };
    let tooltip = opening.tooltip.clone();
    div()
        .id("launch-opening")
        .h_flex()
        .items_center()
        .gap_2()
        .px_3()
        .py_2()
        .rounded_lg()
        .border_1()
        .border_color(theme.border)
        .bg(theme.secondary)
        .text_sm()
        .child(
            div()
                .flex_none()
                .text_color(theme.muted_foreground)
                .child(icon.with_size(px(16.))),
        )
        .child(div().flex_none().child("Opens"))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .font_weight(FontWeight::SEMIBOLD)
                .child(name),
        )
        .when_some(opening.detail.clone(), |strip, detail| {
            strip.child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(detail),
            )
        })
        .when_some(tooltip, |strip, tooltip| {
            strip.tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
        })
}

/// The line under the app name: the running match, or that the app is hidden.
fn subtitle(theme: &Theme, props: &LaunchDialogProps) -> Option<Div> {
    let line = div()
        .h_flex()
        .items_center()
        .gap_1p5()
        .min_w_0()
        .text_xs()
        .text_color(theme.muted_foreground);
    if let Some(running) = &props.running_match {
        let mut text = if props.collaborative {
            "A collaborative session is already running with this storage".to_string()
        } else {
            "Already running with this storage".to_string()
        };
        if let Some(started) = &running.started_label {
            text.push_str(" · ");
            text.push_str(started);
        }
        return Some(
            line.child(
                div()
                    .size(px(6.))
                    .flex_none()
                    .rounded_full()
                    .bg(theme.success),
            )
            .child(div().min_w_0().truncate().child(text)),
        );
    }
    props
        .hidden
        .then(|| line.child(div().truncate().child("Hidden from Library")))
}

fn field_label(theme: &Theme, label: &'static str) -> Div {
    div()
        .text_xs()
        .text_color(theme.muted_foreground)
        .child(Label::new(label))
}

fn select_field(theme: &Theme, label: &'static str, state: &LaunchSelect, enabled: bool) -> Div {
    div()
        .v_flex()
        .gap_1p5()
        .child(field_label(theme, label))
        .child(Select::new(state).disabled(!enabled))
}
