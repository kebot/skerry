use gpui_kit::component::{
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    label::Label,
    select::{SearchableVec, Select, SelectItem, SelectState},
    Disableable as _, IconName, IndexPath, Sizable as _, StyledExt as _, Theme,
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
    /// Shows Favorite / Hide. Off until those choices are saved anywhere (ARCH-GAP-LS-04),
    /// so the dialog never offers a control that does nothing.
    pub show_organize: bool,
    /// The deep link was just copied; the Copy button confirms it in place.
    pub link_copied: bool,
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
    Launch,
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
                        .text_lg()
                        .font_weight(FontWeight::SEMIBOLD)
                        .truncate()
                        .child(props.app_name.clone()),
                ),
        )
        .child(
            div()
                .h_flex()
                .items_center()
                .gap_2()
                .flex_none()
                .when(props.show_organize, |row| {
                    row.child({
                        let mut btn = Button::new("launch-favorite")
                            .small()
                            .icon(if props.favorite {
                                IconName::StarFill
                            } else {
                                IconName::Star
                            })
                            .tooltip(if props.favorite {
                                "Remove Favorite"
                            } else {
                                "Add Favorite"
                            })
                            .disabled(!enabled)
                            .on_click(move |_, window, cx| {
                                on_fav(LaunchDialogAction::ToggleFavorite, window, cx)
                            });
                        if props.favorite {
                            btn = btn.primary();
                        } else {
                            btn = btn.outline();
                        }
                        btn
                    })
                })
                .when(props.show_organize, |row| {
                    row.child({
                        let mut btn = Button::new("launch-hide")
                            .small()
                            .icon(if props.hidden {
                                IconName::Eye
                            } else {
                                IconName::EyeOff
                            })
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
    let footer = div()
        .px_5()
        .py_4()
        .border_t_1()
        .border_color(theme.border)
        .h_flex()
        .items_center()
        .gap_3()
        .when(props.show_copy_link, |footer| {
            footer.child(
                Button::new("launch-copy-link")
                    .outline()
                    .icon(if props.link_copied {
                        IconName::Check
                    } else {
                        IconName::Copy
                    })
                    .label(if props.link_copied {
                        "Copied"
                    } else {
                        "Copy link"
                    })
                    .disabled(!enabled)
                    .on_click(move |_, window, cx| {
                        on_copy(LaunchDialogAction::CopyLink, window, cx)
                    }),
            )
        })
        .child(
            div().flex_1().child(
                Button::new("launch-submit")
                    .primary()
                    .w_full()
                    .icon(IconName::Play)
                    .label(if props.busy { "Launching…" } else { "Launch" })
                    .disabled(!enabled)
                    .on_click(move |_, window, cx| {
                        on_launch(LaunchDialogAction::Launch, window, cx)
                    }),
            ),
        );

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
