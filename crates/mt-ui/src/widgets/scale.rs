//! A diverging colour scale for prices: theme `info` below the centre, `negative`
//! above, muted at the centre. Used by MAP and the GP/SPRD heatmaps.

use egui::{Color32, RichText, Sense, Ui, vec2};

use crate::skin::Skin;
use crate::widgets::fmt;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Diverging {
    pub centre: f64,
    pub half_range: f64,
    /// Whether the centre is zero (components, spreads) or the median (price levels).
    pub zero_centred: bool,
}

impl Diverging {
    /// Fit to the 5th-95th percentile spread around the median (or zero), so a
    /// single spike does not wash out everything else.
    pub fn fit(values: &mut [f64], zero_centred: bool) -> Self {
        if values.is_empty() {
            return Self {
                centre: 0.0,
                half_range: 1.0,
                zero_centred,
            };
        }
        values.sort_by(f64::total_cmp);
        let q = |p: f64| values[((values.len() - 1) as f64 * p).round() as usize];
        let centre = if zero_centred { 0.0 } else { q(0.5) };
        let half_range = (q(0.95) - centre)
            .abs()
            .max((centre - q(0.05)).abs())
            .max(0.5);
        Self {
            centre,
            half_range,
            zero_centred,
        }
    }

    pub fn color(&self, v: f64, skin: &Skin) -> Color32 {
        let t = ((v - self.centre) / self.half_range).clamp(-1.0, 1.0) as f32;
        if t >= 0.0 {
            skin.text_muted.lerp_to_gamma(skin.negative, t)
        } else {
            skin.text_muted.lerp_to_gamma(skin.info, -t)
        }
    }

    /// `≤ lo [gradient] ≥ hi  $/MWh · median X`
    pub fn legend(&self, ui: &mut Ui, skin: &Skin) {
        let (lo, hi) = (self.centre - self.half_range, self.centre + self.half_range);
        ui.label(
            RichText::new(format!("≤ {}", fmt::price(lo)))
                .small()
                .color(skin.info),
        );
        let (rect, _) = ui.allocate_exact_size(vec2(200.0, 10.0), Sense::hover());
        let steps = 40;
        for k in 0..steps {
            let x0 = rect.left() + rect.width() * k as f32 / steps as f32;
            let seg = egui::Rect::from_min_max(
                egui::pos2(x0, rect.top()),
                egui::pos2(x0 + rect.width() / steps as f32 + 0.5, rect.bottom()),
            );
            let v = lo + (hi - lo) * k as f64 / (steps - 1) as f64;
            ui.painter().rect_filled(seg, 0.0, self.color(v, skin));
        }
        ui.label(
            RichText::new(format!("≥ {}", fmt::price(hi)))
                .small()
                .color(skin.negative),
        );
        let centre = if self.zero_centred {
            "centre 0".to_owned()
        } else {
            format!("median {}", fmt::price(self.centre))
        };
        ui.label(
            RichText::new(format!("$/MWh · {centre}"))
                .small()
                .color(skin.text_muted),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centres_and_resists_spikes() {
        let mut v = vec![10.0, 20.0, 30.0, 40.0, 1000.0];
        let s = Diverging::fit(&mut v, false);
        assert_eq!(s.centre, 30.0);
        let mut z = vec![-5.0, 0.0, 2.0];
        let s0 = Diverging::fit(&mut z, true);
        assert_eq!(s0.centre, 0.0);
        assert!(s0.half_range >= 5.0);
        assert_eq!(Diverging::fit(&mut [], false).half_range, 1.0);
    }
}
