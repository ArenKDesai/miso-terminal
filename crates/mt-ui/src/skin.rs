//! Maps an `mt_theme::Theme` onto egui, and gives panels the theme's semantic
//! colours as ready-to-use `Color32`s.

use egui::{Color32, CornerRadius, FontFamily, FontId, Margin, Shadow, Stroke, TextStyle, Visuals};
use mt_theme::{Color, FontSpec, Theme};

/// Named text styles beyond egui's built-ins.
pub const LABEL: &str = "label";
pub const READOUT: &str = "readout";
pub const HEADING: &str = "heading";

pub fn label_style() -> TextStyle {
    TextStyle::Name(LABEL.into())
}

pub fn readout_style() -> TextStyle {
    TextStyle::Name(READOUT.into())
}

pub fn c32(c: Color) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r, c.g, c.b, c.a)
}

/// The active theme plus its palette pre-converted for egui.
#[derive(Clone, Debug)]
pub struct Skin {
    pub theme: Theme,
    pub background: Color32,
    pub surface: Color32,
    pub surface_alt: Color32,
    pub border: Color32,
    pub border_strong: Color32,
    pub text: Color32,
    pub text_strong: Color32,
    pub text_muted: Color32,
    pub accent: Color32,
    pub on_accent: Color32,
    pub live: Color32,
    pub positive: Color32,
    pub negative: Color32,
    pub warning: Color32,
    pub info: Color32,
    pub grid: Color32,
}

impl Skin {
    pub fn new(theme: Theme) -> Self {
        let p = &theme.palette;
        Self {
            background: c32(p.background),
            surface: c32(p.surface),
            surface_alt: c32(p.surface_alt),
            border: c32(p.border),
            border_strong: c32(p.border_strong),
            text: c32(p.text),
            text_strong: c32(p.text_strong),
            text_muted: c32(p.text_muted),
            accent: c32(p.accent),
            on_accent: c32(p.on_accent),
            live: c32(p.live),
            positive: c32(p.positive),
            negative: c32(p.negative),
            warning: c32(p.warning),
            info: c32(p.info),
            grid: c32(theme.grid()),
            theme,
        }
    }

    pub fn series(&self, i: usize) -> Color32 {
        c32(self.theme.series(i))
    }

    pub fn fuel(&self, category: &str) -> Color32 {
        c32(self.theme.fuel_color(mt_core::fuel_key(category)))
    }

    /// Colour for a signed change: up positive, down negative, flat muted.
    pub fn delta(&self, v: f64) -> Color32 {
        if v > 1e-9 {
            self.positive
        } else if v < -1e-9 {
            self.negative
        } else {
            self.text_muted
        }
    }

    /// Apply this skin's colours, geometry and text sizes to an egui style.
    pub fn apply_style(&self, style: &mut egui::Style) {
        let s = &self.theme.style;
        let radius = CornerRadius::same(s.rounding.clamp(0.0, 255.0) as u8);
        let stroke = s.stroke.max(0.0);

        let v: &mut Visuals = &mut style.visuals;
        *v = if self.theme.meta.dark {
            Visuals::dark()
        } else {
            Visuals::light()
        };
        v.override_text_color = None;
        v.panel_fill = self.background;
        v.window_fill = self.surface;
        v.window_stroke = Stroke::new(stroke, self.border_strong);
        v.window_corner_radius = radius;
        v.menu_corner_radius = radius;
        v.extreme_bg_color = self.background;
        v.text_edit_bg_color = Some(self.background);
        v.faint_bg_color = self.surface.lerp_to_gamma(self.surface_alt, 0.35);
        v.code_bg_color = self.surface_alt;
        v.hyperlink_color = self.info;
        v.warn_fg_color = self.warning;
        v.error_fg_color = self.negative;
        v.selection.bg_fill = self.surface_alt;
        v.selection.stroke = Stroke::new(stroke, self.accent);
        v.text_cursor.stroke = Stroke::new(2.0, self.live);
        let off = s.shadow_offset.clamp(0.0, 20.0) as i8;
        let shadow = Shadow {
            offset: [off, off],
            blur: 0,
            spread: 0,
            color: shadow_color(self),
        };
        v.window_shadow = shadow;
        v.popup_shadow = shadow;
        v.striped = true;

        let w = &mut v.widgets;
        for wv in [
            &mut w.noninteractive,
            &mut w.inactive,
            &mut w.hovered,
            &mut w.active,
            &mut w.open,
        ] {
            wv.corner_radius = radius;
            wv.expansion = 0.0;
        }
        w.noninteractive.bg_fill = self.surface;
        w.noninteractive.weak_bg_fill = self.surface;
        w.noninteractive.bg_stroke = Stroke::new(stroke, self.border);
        w.noninteractive.fg_stroke = Stroke::new(stroke, self.text);

        w.inactive.bg_fill = self.surface;
        w.inactive.weak_bg_fill = self.surface;
        w.inactive.bg_stroke = Stroke::new(stroke, self.border_strong);
        w.inactive.fg_stroke = Stroke::new(stroke, self.text);

        w.hovered.bg_fill = self.surface_alt;
        w.hovered.weak_bg_fill = self.surface_alt;
        w.hovered.bg_stroke = Stroke::new(stroke, self.accent);
        w.hovered.fg_stroke = Stroke::new(stroke, self.text_strong);

        w.active.bg_fill = self.accent;
        w.active.weak_bg_fill = self.surface_alt;
        w.active.bg_stroke = Stroke::new(stroke, self.accent);
        w.active.fg_stroke = Stroke::new(stroke, self.text_strong);

        w.open.bg_fill = self.surface_alt;
        w.open.weak_bg_fill = self.surface_alt;
        w.open.bg_stroke = Stroke::new(stroke, self.accent);
        w.open.fg_stroke = Stroke::new(stroke, self.text_strong);

        let sp = &mut style.spacing;
        sp.item_spacing = egui::vec2(s.spacing, (s.spacing * 0.75).max(2.0));
        sp.button_padding = egui::vec2(s.spacing.max(4.0), (s.spacing * 0.5).max(2.0));
        let pad = s.padding.clamp(0.0, 64.0) as i8;
        sp.window_margin = Margin::same(pad);
        sp.menu_margin = Margin::same((pad / 2).max(2));

        let fonts = &self.theme.fonts;
        let size = |f: &Option<FontSpec>, default: f32| f.as_ref().map_or(default, |f| f.size);
        let body = size(&fonts.body, 14.0);
        style.text_styles = [
            (
                TextStyle::Small,
                FontId::new((body * 0.8).round(), FontFamily::Proportional),
            ),
            (TextStyle::Body, FontId::new(body, FontFamily::Proportional)),
            (
                TextStyle::Button,
                FontId::new(body, FontFamily::Proportional),
            ),
            (
                TextStyle::Monospace,
                FontId::new(size(&fonts.mono, 13.0), FontFamily::Monospace),
            ),
            (
                TextStyle::Heading,
                FontId::new(size(&fonts.heading, 17.0), FontFamily::Name(HEADING.into())),
            ),
            (
                label_style(),
                FontId::new(size(&fonts.label, 11.0), FontFamily::Name(LABEL.into())),
            ),
            (
                readout_style(),
                FontId::new(size(&fonts.readout, 26.0), FontFamily::Name(READOUT.into())),
            ),
        ]
        .into();
    }
}

/// A hard shadow a little darker than the ground (Everforge's "shadow-plate").
fn shadow_color(skin: &Skin) -> Color32 {
    let base = if skin.theme.meta.dark {
        Color32::BLACK
    } else {
        skin.border
    };
    skin.background.lerp_to_gamma(base, 0.6)
}
