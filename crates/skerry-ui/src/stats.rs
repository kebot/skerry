//! The Session Controls **Stats** section, drawn as the Selkies web sidebar
//! draws its stats (`selkies-dashboard/src/panels/stats.tsx`, `StatsSection`):
//! status rows, history graphs, figure tiles, and host meters (`ARCH-019`).
//! What each part says is worded by the caller; this module only draws it.

use gpui_kit::component::{
    button::{Button, ButtonVariants as _},
    tooltip::Tooltip,
    IconName, Sizable as _, StyledExt as _, Theme,
};
use gpui_kit::{
    canvas, div, point, prelude::*, px, AnyElement, App, Bounds, Div, FontWeight, Hsla,
    PathBuilder, Pixels, SharedString, Window,
};

/// A graph's height, as the web sidebar's `.stream-graph-plot`.
const GRAPH_HEIGHT: f32 = 44.;
/// The fewest points a graph spreads across its width, so a fresh history
/// grows from the left edge instead of stretching two points edge to edge.
const GRAPH_MIN_POINTS: usize = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Good,
    Warn,
    Neutral,
}

/// One status row: its name, value, and what explains it.
#[derive(Clone, Debug, PartialEq)]
pub struct StatRow {
    pub label: String,
    pub status: Status,
    pub value: String,
    pub detail: String,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Series {
    /// Shown beside the value where a graph has two series ("Client", "Server").
    pub name: String,
    pub values: Vec<f32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Graph {
    pub label: String,
    pub unit: String,
    pub series: Vec<Series>,
    /// The value at the top edge.
    pub max: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Tile {
    pub label: String,
    pub value: String,
    /// The least–most range under the value, where there is one.
    pub detail: String,
    pub warn: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Meter {
    pub label: String,
    /// 0–100, the bar's fill.
    pub percent: f32,
    pub text: String,
    /// A utilization draws a bar; a memory pair shows its amounts instead.
    pub bar: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct StatsProps {
    pub rows: Vec<StatRow>,
    pub graphs: Vec<Graph>,
    pub tiles: Vec<Tile>,
    pub meters: Vec<Meter>,
}

/// The section's body. `copied` shows the copy button's check for a moment
/// after a copy.
pub fn render(
    theme: &Theme,
    props: &StatsProps,
    copied: bool,
    on_copy: impl Fn(&mut Window, &mut App) + 'static,
) -> Div {
    let copy = Button::new("stats-copy")
        .ghost()
        .xsmall()
        .icon(if copied {
            IconName::Check
        } else {
            IconName::Copy
        })
        .tooltip("Copy stats")
        .on_click(move |_, window, cx| on_copy(window, cx));

    div()
        .v_flex()
        .gap(px(10.))
        .text_size(px(13.))
        .child(
            div()
                .h_flex()
                .items_start()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .v_flex()
                        .gap(px(6.))
                        .children(props.rows.iter().map(|row| render_row(theme, row))),
                )
                .child(copy),
        )
        .children(props.graphs.iter().map(|graph| render_graph(theme, graph)))
        .when(!props.tiles.is_empty(), |view| {
            view.child(render_tiles(theme, &props.tiles))
        })
        .when(!props.meters.is_empty(), |view| {
            view.child(
                div()
                    .v_flex()
                    .gap(px(5.))
                    .children(props.meters.iter().map(|meter| render_meter(theme, meter))),
            )
        })
}

fn caption(theme: &Theme, text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(11.))
        .text_color(theme.muted_foreground)
        .child(text.into().to_uppercase())
}

fn status_mark(theme: &Theme, status: Status) -> AnyElement {
    match status {
        Status::Good => div()
            .text_color(theme.success)
            .child(IconName::CircleCheck)
            .into_any_element(),
        Status::Warn => div()
            .text_color(theme.warning)
            .child(IconName::TriangleAlert)
            .into_any_element(),
        Status::Neutral => div()
            .size(px(14.))
            .flex()
            .items_center()
            .justify_center()
            .child(div().size(px(6.)).rounded_full().bg(theme.muted_foreground))
            .into_any_element(),
    }
}

/// A row reads as one text: its mark in the margin, its name running into its
/// value, and its detail and reason as lines of their own.
fn render_row(theme: &Theme, row: &StatRow) -> Div {
    div()
        .h_flex()
        .items_start()
        .gap(px(6.))
        .child(
            div()
                .pt(px(1.))
                .text_size(px(14.))
                .child(status_mark(theme, row.status)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .v_flex()
                .child(
                    div()
                        .h_flex()
                        .flex_wrap()
                        .items_baseline()
                        .gap(px(4.))
                        .child(caption(theme, row.label.clone()))
                        .child(div().font_weight(FontWeight::SEMIBOLD).child(
                            if row.value.is_empty() {
                                "N/A".to_string()
                            } else {
                                row.value.clone()
                            },
                        )),
                )
                .when(!row.detail.is_empty(), |view| {
                    view.child(
                        div()
                            .text_color(theme.muted_foreground)
                            .child(row.detail.clone()),
                    )
                })
                .when(!row.reason.is_empty(), |view| {
                    view.child(
                        div()
                            .mt(px(2.))
                            .pl(px(6.))
                            .border_l_2()
                            .border_color(theme.warning)
                            .text_color(theme.muted_foreground)
                            .child(row.reason.clone()),
                    )
                }),
        )
}

/// The web's `--stat-series-1` (pink) and `--stat-series-2` (violet), in
/// this theme's nearest colors so two series stay apart.
fn series_color(theme: &Theme, index: usize) -> Hsla {
    if index == 0 {
        theme.magenta
    } else {
        theme.blue
    }
}

fn format_value(value: f32) -> String {
    if value.fract().abs() < 0.05 {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    }
}

/// One graph: its name, the newest value of each series, and the series
/// against one axis, with the axis top in the corner.
fn render_graph(theme: &Theme, graph: &Graph) -> Div {
    let values = div()
        .h_flex()
        .gap(px(10.))
        .text_color(theme.muted_foreground)
        .children(graph.series.iter().enumerate().map(|(index, series)| {
            let newest = series.values.last().copied().unwrap_or(0.);
            div()
                .h_flex()
                .items_baseline()
                .gap(px(4.))
                .when(graph.series.len() > 1, |view| {
                    view.child(
                        div()
                            .w(px(10.))
                            .h(px(2.))
                            .rounded(px(1.))
                            .bg(series_color(theme, index)),
                    )
                })
                .child(
                    div()
                        .text_size(px(14.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme.foreground)
                        .child(format_value(newest)),
                )
                .child(if graph.series.len() > 1 {
                    series.name.clone()
                } else {
                    graph.unit.clone()
                })
        }));

    let plot = {
        let series: Vec<(Vec<f32>, Hsla)> = graph
            .series
            .iter()
            .enumerate()
            .map(|(index, series)| (series.values.clone(), series_color(theme, index)))
            .collect();
        let max = graph.max;
        let grid = theme.border;
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| paint_graph(bounds, &series, max, grid, window),
        )
        .size_full()
    };

    div()
        .v_flex()
        .child(
            div()
                .h_flex()
                .items_baseline()
                .justify_between()
                .gap_2()
                .mb(px(2.))
                .child(caption(theme, graph.label.clone()))
                .child(values),
        )
        .child(
            div()
                .relative()
                .w_full()
                .h(px(GRAPH_HEIGHT))
                .rounded(px(6.))
                .overflow_hidden()
                .bg(theme.background)
                .child(plot)
                .child(
                    div()
                        .absolute()
                        .right(px(4.))
                        .top(px(2.))
                        .px(px(4.))
                        .rounded(px(3.))
                        .bg(theme.background)
                        .text_size(px(11.))
                        .text_color(theme.muted_foreground)
                        .child(format!(
                            "{} {}",
                            format_value(graph.max.round()),
                            graph.unit
                        )),
                ),
        )
}

/// The web's `graphPath`: points spread over at least `GRAPH_MIN_POINTS`, a
/// line per series, and the first series filled under at 12%.
fn paint_graph(
    bounds: Bounds<Pixels>,
    series: &[(Vec<f32>, Hsla)],
    max: f32,
    grid: Hsla,
    window: &mut Window,
) {
    let width = f32::from(bounds.size.width);
    let height = f32::from(bounds.size.height);
    let origin = bounds.origin;
    let at = |x: f32, y: f32| point(origin.x + px(x), origin.y + px(y));

    let mut midline = PathBuilder::stroke(px(1.));
    midline.move_to(at(0., height / 2.));
    midline.line_to(at(width, height / 2.));
    if let Ok(path) = midline.build() {
        window.paint_path(path, grid);
    }

    let top = if max > 0. { max } else { 1. };
    for (index, (values, color)) in series.iter().enumerate() {
        if values.is_empty() {
            continue;
        }
        let step = width / (values.len().max(GRAPH_MIN_POINTS) - 1) as f32;
        let points: Vec<_> = values
            .iter()
            .enumerate()
            .map(|(i, value)| {
                let y = height - 1. - (value.clamp(0., top) / top) * (height - 2.);
                at(i as f32 * step, y)
            })
            .collect();
        if index == 0 && points.len() > 1 {
            let mut area = PathBuilder::fill();
            area.move_to(points[0]);
            for p in &points[1..] {
                area.line_to(*p);
            }
            area.line_to(at((points.len() - 1) as f32 * step, height));
            area.line_to(at(0., height));
            area.close();
            if let Ok(path) = area.build() {
                window.paint_path(path, color.opacity(0.12));
            }
        }
        let mut line = PathBuilder::stroke(px(2.));
        line.move_to(points[0]);
        for p in &points[1..] {
            line.line_to(*p);
        }
        if points.len() == 1 {
            line.line_to(points[0] + point(px(1.), px(0.)));
        }
        if let Ok(path) = line.build() {
            window.paint_path(path, *color);
        }
    }
}

/// The figures as a two-column grid of tiles: value, its range, and its name.
fn render_tiles(theme: &Theme, tiles: &[Tile]) -> Div {
    div()
        .v_flex()
        .gap(px(6.))
        .children(tiles.chunks(2).map(|pair| {
            div()
                .h_flex()
                .items_stretch()
                .gap(px(6.))
                .children(pair.iter().map(|tile| render_tile(theme, tile)))
                .when(pair.len() == 1, |row| row.child(div().flex_1()))
        }))
}

fn render_tile(theme: &Theme, tile: &Tile) -> AnyElement {
    let body = div()
        .size_full()
        .v_flex()
        .px(px(7.))
        .py(px(5.))
        .rounded(px(6.))
        .border_1()
        .border_color(if tile.warn {
            theme.warning
        } else {
            theme.border
        })
        .bg(theme.background)
        .child(
            div()
                .h_flex()
                .items_center()
                .gap(px(3.))
                .font_weight(FontWeight::BOLD)
                .when(tile.warn, |view| {
                    view.child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme.warning)
                            .child(IconName::TriangleAlert),
                    )
                })
                .child(tile.value.clone()),
        )
        .when(!tile.detail.is_empty(), |view| {
            view.child(
                div()
                    .text_size(px(10.5))
                    .text_color(theme.muted_foreground)
                    .child(tile.detail.clone()),
            )
        })
        .child(
            div()
                .text_size(px(11.))
                .text_color(theme.muted_foreground)
                .child(tile.label.clone()),
        );
    let cell = div()
        .id(SharedString::from(format!("tile-{}", tile.label)))
        .flex_1()
        .min_w_0()
        .child(body);
    if tile.warn {
        cell.tooltip(|window, cx| {
            Tooltip::new("More than 10% over the target rate").build(window, cx)
        })
        .into_any_element()
    } else {
        cell.into_any_element()
    }
}

/// A host meter: its name, a bar for a utilization, and its figure.
fn render_meter(theme: &Theme, meter: &Meter) -> Div {
    let row = div()
        .h_flex()
        .items_center()
        .gap(px(8.))
        .child(caption(theme, meter.label.clone()).w(px(78.)).flex_none());
    if meter.bar {
        let fill = meter.percent.clamp(0., 100.) / 100.;
        row.child(
            div()
                .flex_1()
                .h(px(6.))
                .rounded(px(3.))
                .border_1()
                .border_color(theme.border)
                .bg(theme.background)
                .overflow_hidden()
                .child(
                    div()
                        .h_full()
                        .w(gpui_kit::relative(fill))
                        .rounded(px(3.))
                        .bg(theme.primary),
                ),
        )
        .child(
            div()
                .w(px(40.))
                .flex_none()
                .text_right()
                .text_color(theme.muted_foreground)
                .child(meter.text.clone()),
        )
    } else {
        row.child(
            div()
                .flex_1()
                .text_color(theme.muted_foreground)
                .child(meter.text.clone()),
        )
    }
}
