//! Copy or save a panel as an image (right-click a tab title).
//!
//! egui screenshots the whole window and hands the image back a frame or two
//! later as an `Event::Screenshot`; this module asks for one tagged with the
//! panel's rectangle, then crops and delivers it.

use std::path::{Path, PathBuf};

use egui::{ColorImage, Context, Event, Rect, UserData, ViewportCommand};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Copy,
    SavePng,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    /// The panel's area in points.
    pub rect: Rect,
    pub action: Action,
    /// Used for the file name, e.g. `GP MINN.HUB`.
    pub name: String,
}

pub fn send(ctx: &Context, request: Request) {
    ctx.send_viewport_cmd(ViewportCommand::Screenshot(UserData::new(request)));
}

/// Deliver any screenshots that arrived this frame. Returns a message per
/// capture (`Err` for failures) for the command-line feedback.
pub fn deliver(ctx: &Context, exports_dir: &Path) -> Vec<Result<String, String>> {
    let shots: Vec<(Request, std::sync::Arc<ColorImage>)> = ctx.input(|i| {
        i.events
            .iter()
            .filter_map(|e| match e {
                Event::Screenshot {
                    user_data, image, ..
                } => {
                    let req = user_data.data.as_ref()?.downcast_ref::<Request>()?.clone();
                    Some((req, image.clone()))
                }
                _ => None,
            })
            .collect()
    });
    let ppp = ctx.pixels_per_point();
    shots
        .into_iter()
        .map(|(req, image)| {
            let crop = image.region(&req.rect, Some(ppp));
            match req.action {
                Action::Copy => {
                    ctx.copy_image(crop);
                    Ok(format!("Copied {} to the clipboard", req.name))
                }
                Action::SavePng => save_png(&crop, exports_dir, &req.name)
                    .map(|p| format!("Saved {}", p.display()))
                    .map_err(|e| format!("Could not save image: {e}")),
            }
        })
        .collect()
}

/// `GP MINN.HUB 14` -> `GP-MINN.HUB-14`
fn file_stem(name: &str) -> String {
    name.split_whitespace()
        .map(|w| {
            w.chars()
                .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
                .collect::<String>()
        })
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

fn save_png(image: &ColorImage, dir: &Path, name: &str) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let path = dir.join(format!("{}-{stamp}.png", file_stem(name)));
    let [w, h] = image.size;
    let rgba: Vec<u8> = image
        .pixels
        .iter()
        .flat_map(egui::Color32::to_srgba_unmultiplied)
        .collect();
    let buf = image::RgbaImage::from_raw(w as u32, h as u32, rgba).ok_or("image size mismatch")?;
    buf.save(&path).map_err(|e| e.to_string())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_are_safe() {
        assert_eq!(file_stem("GP MINN.HUB 14"), "GP-MINN.HUB-14");
        assert_eq!(file_stem("SPRD A/B <x>"), "SPRD-AB-x");
    }

    #[test]
    fn saves_a_png() {
        let dir = std::env::temp_dir().join(format!("mt-capture-{}", std::process::id()));
        let img = ColorImage::new([4, 3], vec![egui::Color32::from_rgb(10, 20, 30); 12]);
        let path = save_png(&img, &dir, "TEST").unwrap();
        let back = image::open(&path).unwrap().to_rgba8();
        assert_eq!(back.dimensions(), (4, 3));
        assert_eq!(back.get_pixel(0, 0).0, [10, 20, 30, 255]);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
