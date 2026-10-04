//! The theme gallery as a data source: the index the docs site publishes
//! (`mt_theme::gallery`), fetched through the hub like any other feed so THEME
//! can list the themes and install one with a click.

use std::sync::Arc;
use std::time::Duration;

use mt_data::{FetchCtx, FetchError, Freshness, Query};
use mt_theme::Gallery;
use mt_theme::gallery::{GALLERY_INDEX_URL, parse_index};

/// The gallery changes when the docs site is redeployed; a few times a day is plenty.
pub const GALLERY_REFRESH: Duration = Duration::from_secs(6 * 60 * 60);

/// The published theme gallery.
#[derive(Clone, Debug)]
pub struct GalleryQuery {
    url: Arc<str>,
}

impl Default for GalleryQuery {
    fn default() -> Self {
        Self {
            url: GALLERY_INDEX_URL.into(),
        }
    }
}

impl Query for GalleryQuery {
    type Output = Gallery;

    fn key(&self) -> String {
        "themes/gallery".into()
    }

    fn label(&self) -> String {
        "Theme gallery (docs site)".into()
    }

    fn freshness(&self, _: &Gallery) -> Freshness {
        Freshness::Every(GALLERY_REFRESH)
    }

    async fn fetch(
        &self,
        ctx: FetchCtx,
        _prev: Option<Arc<Gallery>>,
    ) -> Result<Gallery, FetchError> {
        let body = ctx.get_text(&*self.url).await?;
        parse_index(&body).map_err(|e| FetchError::parse("theme gallery", e))
    }
}
