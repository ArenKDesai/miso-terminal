use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// An sRGB colour, written in theme files as `#rrggbb` or `#rrggbbaa`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    pub const fn with_alpha(self, a: u8) -> Self {
        Self { a, ..self }
    }

    /// Linear blend towards `other`; `t = 0` is `self`, `t = 1` is `other`.
    pub fn mix(self, other: Self, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        let lerp = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
        Self {
            r: lerp(self.r, other.r),
            g: lerp(self.g, other.g),
            b: lerp(self.b, other.b),
            a: lerp(self.a, other.a),
        }
    }

    /// WCAG 2.x relative luminance.
    pub fn luminance(self) -> f32 {
        let ch = |c: u8| {
            let c = f32::from(c) / 255.0;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * ch(self.r) + 0.7152 * ch(self.g) + 0.0722 * ch(self.b)
    }

    pub fn to_hex(self) -> String {
        if self.a == 255 {
            format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
        } else {
            format!("#{:02x}{:02x}{:02x}{:02x}", self.r, self.g, self.b, self.a)
        }
    }
}

/// WCAG contrast ratio between two opaque colours (1.0 to 21.0).
pub fn contrast_ratio(a: Color, b: Color) -> f32 {
    let (la, lb) = (a.luminance(), b.luminance());
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

#[derive(Debug, thiserror::Error)]
#[error("invalid colour {0:?}: expected #rrggbb or #rrggbbaa")]
pub struct ColorParseError(String);

impl FromStr for Color {
    type Err = ColorParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || ColorParseError(s.to_owned());
        let hex = s.trim().strip_prefix('#').ok_or_else(err)?;
        if !hex.is_ascii() {
            return Err(err());
        }
        let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|_| err());
        match hex.len() {
            6 => Ok(Self::rgb(byte(0)?, byte(2)?, byte(4)?)),
            8 => Ok(Self::rgb(byte(0)?, byte(2)?, byte(4)?).with_alpha(byte(6)?)),
            _ => Err(err()),
        }
    }
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl Serialize for Color {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_prints_hex() {
        let c: Color = "#a7c080".parse().unwrap();
        assert_eq!(c, Color::rgb(0xa7, 0xc0, 0x80));
        assert_eq!(c.to_hex(), "#a7c080");
        let t: Color = "#6ef0b066".parse().unwrap();
        assert_eq!(t.a, 0x66);
        assert_eq!(t.to_hex(), "#6ef0b066");
        assert!("a7c080".parse::<Color>().is_err());
        assert!("#a7c08".parse::<Color>().is_err());
        assert!("#zzzzzz".parse::<Color>().is_err());
    }

    #[test]
    fn contrast_matches_wcag_reference_values() {
        let (black, white) = (Color::rgb(0, 0, 0), Color::rgb(255, 255, 255));
        assert!((contrast_ratio(black, white) - 21.0).abs() < 0.01);
        assert!((contrast_ratio(white, white) - 1.0).abs() < 0.01);
        // #767676 on white is the classic 4.54:1.
        assert!((contrast_ratio(Color::rgb(0x76, 0x76, 0x76), white) - 4.54).abs() < 0.02);
    }

    #[test]
    fn mixes() {
        let m = Color::rgb(0, 0, 0).mix(Color::rgb(200, 100, 50), 0.5);
        assert_eq!(m, Color::rgb(100, 50, 25));
    }
}
