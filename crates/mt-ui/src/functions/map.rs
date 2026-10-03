//! MAP: prices across the footprint. Every node MISO plots, coloured by the
//! chosen metric on a diverging scale; hover for details, click to graph.

use egui::{Color32, Pos2, RichText, Shape, Stroke, Ui, vec2};
use egui_plot::{Line, Plot, PlotPoint, PlotPoints};
use mt_core::{LmpBoardRow, hub_short};

use crate::context::PanelCx;
use crate::function::{Category, FunctionSpec, Panel, Route};
use crate::geo::{self, GeoNode};
use crate::skin::{Skin, label_style};
use crate::widgets::scale::Diverging;
use crate::widgets::{self, fmt};

pub const SPEC: FunctionSpec = FunctionSpec {
    code: "MAP",
    aliases: &["HEAT", "GEO"],
    name: "Price map",
    category: Category::Prices,
    usage: "MAP [LMP|MCC|MLC|DA|DART]",
    description: "Prices across the MISO footprint: every mapped node coloured by RT LMP, congestion, loss, DA or RT − DA. Click a node to graph it.",
    takes_node: false,
    open,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Metric {
    Lmp,
    Congestion,
    Loss,
    Da,
    Dart,
}

impl Metric {
    const ALL: [Self; 5] = [
        Self::Lmp,
        Self::Congestion,
        Self::Loss,
        Self::Da,
        Self::Dart,
    ];

    fn code(self) -> &'static str {
        match self {
            Self::Lmp => "LMP",
            Self::Congestion => "MCC",
            Self::Loss => "MLC",
            Self::Da => "DA",
            Self::Dart => "DART",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Lmp => "RT LMP",
            Self::Congestion => "Congestion",
            Self::Loss => "Loss",
            Self::Da => "DA ex-post",
            Self::Dart => "RT − DA",
        }
    }

    fn value(self, r: &LmpBoardRow) -> Option<f64> {
        match self {
            Self::Lmp => r.rt_5min.map(|p| p.lmp),
            Self::Congestion => r.rt_5min.map(|p| p.mcc),
            Self::Loss => r.rt_5min.map(|p| p.mlc),
            Self::Da => r.da_expost.map(|p| p.lmp),
            Self::Dart => r.dart(),
        }
    }

    /// Price levels centre on the median; components and spreads on zero.
    fn centred_on_zero(self) -> bool {
        matches!(self, Self::Congestion | Self::Loss | Self::Dart)
    }
}

fn open(args: &[String]) -> Result<Box<dyn Panel>, String> {
    let metric = match args.first().map(|a| a.to_ascii_uppercase()) {
        None => Metric::Lmp,
        Some(a) => Metric::ALL
            .into_iter()
            .find(|m| m.code() == a)
            .ok_or_else(|| format!("unknown metric {a}; use LMP, MCC, MLC, DA or DART"))?,
    };
    Ok(Box::new(MapPanel {
        metric,
        show_generators: true,
        labels: true,
    }))
}

struct MapPanel {
    metric: Metric,
    show_generators: bool,
    labels: bool,
}

impl Panel for MapPanel {
    fn title(&self) -> String {
        format!("MAP {}", self.metric.code())
    }

    fn route(&self) -> Route {
        Route::new("MAP", [self.metric.code()])
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>) {
        let skin = cx.skin;
        let board = cx.hub.watch(&cx.miso.lmp_board());
        ui.horizontal(|ui| {
            for m in Metric::ALL {
                ui.selectable_value(&mut self.metric, m, m.label());
            }
            ui.separator();
            ui.checkbox(&mut self.show_generators, "Generators");
            ui.checkbox(&mut self.labels, "Hub labels");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                widgets::freshness(ui, skin, &board)
            });
        });
        let Some(board) = board.data() else {
            widgets::placeholder(ui, skin, board.error.as_ref().map(ToString::to_string));
            return;
        };
        let map = geo::map();
        let visible = |n: &GeoNode| self.show_generators || n.kind != "Generator";
        let points: Vec<(&GeoNode, [f64; 2], Option<f64>)> = map
            .nodes
            .iter()
            .filter(|n| visible(n))
            .map(|n| {
                (
                    n,
                    geo::project(n.lon, n.lat),
                    board.row(&n.node).and_then(|r| self.metric.value(r)),
                )
            })
            .collect();
        let mut values: Vec<f64> = points.iter().filter_map(|p| p.2).collect();
        let scale = Diverging::fit(&mut values, self.metric.centred_on_zero());
        legend(ui, skin, &scale, board.interval);

        let outline = |ring: &[[f64; 2]], color: Color32, width: f32| {
            let pts: Vec<[f64; 2]> = ring.iter().map(|p| geo::project(p[0], p[1])).collect();
            Line::new("", PlotPoints::from(pts))
                .color(color)
                .width(width)
                .allow_hover(false)
        };
        // Frame every visible node, including interfaces outside the footprint.
        let (mut x0, mut x1, mut y0, mut y1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
        for (_, [x, y], _) in &points {
            (x0, x1, y0, y1) = (x0.min(*x), x1.max(*x), y0.min(*y), y1.max(*y));
        }
        let resp = Plot::new("miso-map")
            .include_x(x0)
            .include_x(x1)
            .include_y(y0)
            .include_y(y1)
            .data_aspect(1.0)
            .show_axes(false)
            .show_grid(false)
            .show_x(false)
            .show_y(false)
            .label_formatter(|_| None)
            .allow_double_click_reset(true)
            .show(ui, |plot| {
                for ring in &map.states {
                    plot.line(outline(ring, skin.border, 1.0));
                }
                for ring in &map.footprint {
                    plot.line(outline(ring, skin.border_strong, 1.5));
                }
            });

        // Markers are painted in screen space over the plot so each can have its
        // own colour and shape, and so hover hit-testing is exact.
        let painter = ui.painter_at(resp.response.rect);
        let hover = resp.response.hover_pos();
        let mut nearest: Option<(f32, usize)> = None;
        // Generators first so hubs and zones sit on top.
        let mut order: Vec<usize> = (0..points.len()).collect();
        order.sort_by_key(|&i| match points[i].0.kind.as_str() {
            "Generator" => 0,
            "Interface" => 1,
            "Load zone" => 2,
            _ => 3,
        });
        for &i in &order {
            let (node, xy, value) = &points[i];
            let pos = resp
                .transform
                .position_from_point(&PlotPoint::new(xy[0], xy[1]));
            let fill = value.map_or(skin.border_strong, |v| scale.color(v, skin));
            let outline = Stroke::new(1.0, skin.background);
            match node.kind.as_str() {
                "Hub" => {
                    painter.add(diamond(pos, 7.0, fill, Stroke::new(1.5, skin.text_strong)));
                    if self.labels {
                        // Pairs of hubs sit close together; label the western one on its left.
                        let (offset, align) = if LEFT_LABELS.contains(&node.node.as_str()) {
                            (vec2(-10.0, -1.0), egui::Align2::RIGHT_CENTER)
                        } else {
                            (vec2(10.0, -1.0), egui::Align2::LEFT_CENTER)
                        };
                        let font = ui.style().text_styles[&label_style()].clone();
                        let galley = painter.layout_no_wrap(
                            hub_short(&node.node).to_owned(),
                            font,
                            skin.text_strong,
                        );
                        let rect = align.anchor_size(pos + offset, galley.size());
                        painter.rect_filled(
                            rect.expand(2.0),
                            2.0,
                            skin.background.gamma_multiply(0.8),
                        );
                        painter.galley(rect.min, galley, skin.text_strong);
                    }
                }
                "Load zone" => {
                    painter.rect(
                        egui::Rect::from_center_size(pos, vec2(8.0, 8.0)),
                        1.0,
                        fill,
                        outline,
                        egui::StrokeKind::Outside,
                    );
                }
                "Interface" => {
                    let r = 5.0;
                    painter.add(Shape::convex_polygon(
                        vec![
                            pos + vec2(0.0, -r),
                            pos + vec2(r, r * 0.8),
                            pos + vec2(-r, r * 0.8),
                        ],
                        fill,
                        outline,
                    ));
                }
                _ => {
                    painter.circle(pos, 3.5, fill, outline);
                }
            }
            if let Some(h) = hover {
                let d = h.distance(pos);
                if d < 12.0 && nearest.is_none_or(|(best, _)| d < best) {
                    nearest = Some((d, i));
                }
            }
        }

        if let Some((_, i)) = nearest {
            let (node, xy, value) = &points[i];
            let pos = resp
                .transform
                .position_from_point(&PlotPoint::new(xy[0], xy[1]));
            painter.circle_stroke(pos, 10.0, Stroke::new(2.0, skin.live));
            let row = board.row(&node.node);
            let response = resp.response.clone().on_hover_ui_at_pointer(|ui| {
                ui.label(RichText::new(&node.node).strong().color(skin.text_strong));
                ui.label(RichText::new(&node.kind).small().color(skin.text_muted));
                ui.label(format!(
                    "{}: {}",
                    self.metric.label(),
                    fmt::price_opt(*value)
                ));
                if let Some(p) = row.and_then(|r| r.rt_5min) {
                    ui.label(
                        RichText::new(format!(
                            "RT {} = energy {} + MCC {} + MLC {}",
                            fmt::price(p.lmp),
                            fmt::price(p.energy()),
                            fmt::price(p.mcc),
                            fmt::price(p.mlc)
                        ))
                        .small(),
                    );
                }
                ui.label(
                    RichText::new("click to graph")
                        .small()
                        .color(skin.text_muted),
                );
            });
            if response.clicked() {
                cx.open(Route::new("GP", [node.node.clone()]));
            }
        }
    }
}

/// Hubs labelled on their left so neighbouring labels don't collide.
const LEFT_LABELS: [&str; 2] = ["ILLINOIS.HUB", "TEXAS.HUB"];

fn diamond(c: Pos2, r: f32, fill: Color32, stroke: Stroke) -> Shape {
    Shape::convex_polygon(
        vec![
            c + vec2(0.0, -r),
            c + vec2(r, 0.0),
            c + vec2(0.0, r),
            c + vec2(-r, 0.0),
        ],
        fill,
        stroke,
    )
}

/// The colour bar, the interval it shows, and the marker key.
fn legend(ui: &mut Ui, skin: &Skin, scale: &Diverging, interval: Option<chrono::NaiveDateTime>) {
    ui.horizontal(|ui| {
        scale.legend(ui, skin);
        if let Some(t) = interval {
            ui.label(
                RichText::new(format!("· RT interval {} EST", fmt::hm(t)))
                    .small()
                    .color(skin.text_muted),
            );
        }
        ui.label(
            RichText::new("◆ hub  ■ load zone  ▲ interface  ● generator")
                .small()
                .color(skin.text_muted),
        );
    });
}
