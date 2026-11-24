//! Visible-page `+/-1` virtualization for large documents.
//!
//! A 500-page textbook must never sit in memory as bitmaps. [`PageWindowCache`] keeps rendered
//! pages only for the visible range plus one page on each side, under an explicit byte cap, and
//! evicts everything else as the reader scrolls.

use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::Arc;

use crate::pdf::{PdfError, with_pdf_renderer};

/// Pages kept warm on each side of the visible range.
pub const PAGE_MARGIN: usize = 1;

/// The pages worth holding in memory: `visible` widened by [`PAGE_MARGIN`], clamped to the
/// document. A hostile or empty `visible` range yields an empty window.
#[must_use]
pub fn window_pages(visible: &Range<usize>, page_count: usize) -> Range<usize> {
    if page_count == 0 || visible.start >= visible.end || visible.start >= page_count {
        return 0..0;
    }
    let start = visible.start.saturating_sub(PAGE_MARGIN);
    let end = visible
        .end
        .min(page_count)
        .saturating_add(PAGE_MARGIN)
        .min(page_count);
    start..end
}

/// One cached page bitmap (straight RGBA8).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageBitmap {
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<[u8]>,
}

impl PageBitmap {
    #[must_use]
    pub fn byte_len(&self) -> usize {
        self.rgba.len()
    }
}

/// Why a bitmap was not cached.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CacheError {
    /// The page is outside the current window and would be evicted immediately.
    OutsideWindow { page: usize },
    /// The bitmap cannot fit the byte cap even after evicting everything evictable.
    OverBudget { needed: usize, cap: usize },
    /// Zero-sized bitmap or a buffer that does not match `width * height * 4`.
    BadBitmap,
}

/// Byte-capped cache of rendered pages for the visible window.
#[derive(Debug)]
pub struct PageWindowCache {
    cap_bytes: usize,
    visible: Range<usize>,
    window: Range<usize>,
    pages: BTreeMap<usize, PageBitmap>,
    bytes: usize,
    refused: usize,
}

impl PageWindowCache {
    #[must_use]
    pub fn new(cap_bytes: usize) -> Self {
        Self {
            cap_bytes,
            visible: 0..0,
            window: 0..0,
            pages: BTreeMap::new(),
            bytes: 0,
            refused: 0,
        }
    }

    #[must_use]
    pub fn cap_bytes(&self) -> usize {
        self.cap_bytes
    }

    #[must_use]
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.pages.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pages.is_empty()
    }

    /// Number of bitmaps refused because they exceeded the byte cap.
    #[must_use]
    pub fn refused(&self) -> usize {
        self.refused
    }

    #[must_use]
    pub fn window(&self) -> Range<usize> {
        self.window.clone()
    }

    /// Moves the viewport and evicts every page that fell outside `visible +/- 1`.
    /// Returns the number of evicted pages.
    pub fn set_visible(&mut self, visible: Range<usize>, page_count: usize) -> usize {
        self.window = window_pages(&visible, page_count);
        self.visible = if self.window.is_empty() {
            0..0
        } else {
            visible
        };
        let stale: Vec<usize> = self
            .pages
            .keys()
            .copied()
            .filter(|p| !self.window.contains(p))
            .collect();
        for page in &stale {
            self.remove(*page);
        }
        stale.len()
    }

    fn remove(&mut self, page: usize) {
        if let Some(old) = self.pages.remove(&page) {
            self.bytes = self.bytes.saturating_sub(old.byte_len());
        }
    }

    /// Pages inside the window that still need rendering, visible pages first.
    #[must_use]
    pub fn missing(&self) -> Vec<usize> {
        let mut missing: Vec<usize> = self
            .window
            .clone()
            .filter(|p| !self.pages.contains_key(p))
            .collect();
        missing.sort_by_key(|p| usize::from(!self.visible.contains(p)));
        missing
    }

    #[must_use]
    pub fn get(&self, page: usize) -> Option<&PageBitmap> {
        self.pages.get(&page)
    }

    /// Distance from `page` to the visible range (0 when visible).
    fn distance(&self, page: usize) -> usize {
        if page < self.visible.start {
            self.visible.start - page
        } else if page >= self.visible.end {
            page - self.visible.end + 1
        } else {
            0
        }
    }

    /// Caches a rendered page. Margin pages are evicted (farthest first) to make room, but a
    /// page is never admitted by evicting a page closer to the viewport than itself.
    pub fn insert(&mut self, page: usize, bitmap: PageBitmap) -> Result<(), CacheError> {
        let expected = (bitmap.width as usize)
            .checked_mul(bitmap.height as usize)
            .and_then(|px| px.checked_mul(4));
        if bitmap.width == 0 || bitmap.height == 0 || expected != Some(bitmap.rgba.len()) {
            return Err(CacheError::BadBitmap);
        }
        if !self.window.contains(&page) {
            return Err(CacheError::OutsideWindow { page });
        }
        let needed = bitmap.byte_len();
        self.remove(page);
        let own_distance = self.distance(page);
        while self.bytes.saturating_add(needed) > self.cap_bytes {
            let victim = self
                .pages
                .keys()
                .copied()
                .max_by_key(|p| self.distance(*p))
                .filter(|p| self.distance(*p) > own_distance);
            let Some(victim) = victim else {
                self.refused += 1;
                return Err(CacheError::OverBudget {
                    needed,
                    cap: self.cap_bytes,
                });
            };
            self.remove(victim);
        }
        self.bytes = self.bytes.saturating_add(needed);
        self.pages.insert(page, bitmap);
        Ok(())
    }

    /// Renders every missing window page of an embedded PDF (parsing it once), optionally
    /// inverting for dark reading. Pages that exceed the byte cap are skipped, not buffered.
    /// Returns how many pages were cached.
    pub fn fill_from_pdf(
        &mut self,
        pdf: Arc<Vec<u8>>,
        max_width: u32,
        max_height: u32,
        dark: bool,
    ) -> Result<usize, PdfError> {
        let worst_case = (u64::from(max_width) * u64::from(max_height)).saturating_mul(4);
        if worst_case > self.cap_bytes as u64 {
            self.refused += 1;
            return Ok(0);
        }
        let todo = self.missing();
        with_pdf_renderer(pdf, |renderer| {
            let mut cached = 0;
            for page in todo {
                let Ok(rendered) = renderer.render_page_rgba(page, max_width, max_height) else {
                    continue;
                };
                let mut rgba = rendered.rgba;
                if dark {
                    drawcraft_color::invert_rgba8_buffer_for_dark(&mut rgba);
                }
                let bitmap = PageBitmap {
                    width: rendered.width,
                    height: rendered.height,
                    rgba: Arc::from(rgba),
                };
                if self.insert(page, bitmap).is_ok() {
                    cached += 1;
                }
            }
            cached
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bitmap(w: u32, h: u32) -> PageBitmap {
        PageBitmap {
            width: w,
            height: h,
            rgba: Arc::from(vec![255u8; (w * h * 4) as usize]),
        }
    }

    /// Builds a valid blank N-page PDF.
    fn blank_pdf(pages: usize) -> Vec<u8> {
        let mut objects: Vec<String> = vec!["<< /Type /Catalog /Pages 2 0 R >>".to_string()];
        let kids: Vec<String> = (0..pages).map(|i| format!("{} 0 R", 3 + i)).collect();
        objects.push(format!(
            "<< /Type /Pages /Kids [{}] /Count {pages} >>",
            kids.join(" ")
        ));
        for _ in 0..pages {
            objects.push(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Resources << >> >>"
                    .to_string(),
            );
        }
        let mut pdf = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, body) in objects.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
        }
        let xref = pdf.len();
        pdf.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
        );
        for off in offsets {
            pdf.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
        }
        pdf.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .as_bytes(),
        );
        pdf
    }

    #[test]
    fn window_is_visible_plus_one_each_side_clamped_to_the_document() {
        assert_eq!(window_pages(&(10..12), 500), 9..13);
        assert_eq!(window_pages(&(0..1), 500), 0..2);
        assert_eq!(window_pages(&(499..500), 500), 498..500);
        assert_eq!(window_pages(&(0..1), 1), 0..1);
    }

    #[test]
    fn hostile_visible_ranges_give_an_empty_window() {
        assert_eq!(window_pages(&(5..5), 10), 0..0);
        #[allow(clippy::reversed_empty_ranges)]
        let reversed = 9..3;
        assert_eq!(window_pages(&reversed, 10), 0..0);
        assert_eq!(window_pages(&(20..30), 10), 0..0);
        assert_eq!(window_pages(&(0..usize::MAX), 10), 0..10);
        assert_eq!(window_pages(&(usize::MAX - 1..usize::MAX), 10), 0..0);
        assert_eq!(window_pages(&(0..5), 0), 0..0);
    }

    #[test]
    fn scrolling_through_a_500_page_book_never_holds_more_than_three_pages()
    -> Result<(), CacheError> {
        let mut cache = PageWindowCache::new(1_000_000);
        let mut peak = 0;
        for top in 0..500 {
            cache.set_visible(top..top + 1, 500);
            for page in cache.missing() {
                cache.insert(page, bitmap(10, 10))?;
            }
            peak = peak.max(cache.len());
            assert!(cache.len() <= 3);
        }
        assert_eq!(peak, 3);
        assert_eq!(cache.window(), 498..500);
        assert_eq!(cache.bytes(), 2 * 400);
        Ok(())
    }

    #[test]
    fn moving_the_viewport_evicts_pages_that_left_the_window() -> Result<(), CacheError> {
        let mut cache = PageWindowCache::new(1_000_000);
        cache.set_visible(5..7, 100);
        for p in cache.missing() {
            cache.insert(p, bitmap(8, 8))?;
        }
        assert_eq!(cache.len(), 4);
        let evicted = cache.set_visible(50..52, 100);
        assert_eq!(evicted, 4);
        assert!(cache.is_empty() && cache.bytes() == 0);
        Ok(())
    }

    #[test]
    fn byte_cap_is_enforced_by_dropping_margin_pages_first() -> Result<(), CacheError> {
        // 3 pages of 400 bytes need 1200; cap 900 keeps the visible page plus one margin page.
        let mut cache = PageWindowCache::new(900);
        cache.set_visible(10..11, 100);
        cache.insert(10, bitmap(10, 10))?;
        cache.insert(9, bitmap(10, 10))?;
        // A margin page never evicts a page that is as close or closer to the viewport.
        assert_eq!(
            cache.insert(11, bitmap(10, 10)),
            Err(CacheError::OverBudget {
                needed: 400,
                cap: 900
            })
        );
        assert!(cache.bytes() <= 900);
        assert!(
            cache.get(10).is_some(),
            "visible page is never sacrificed for a margin page"
        );
        assert_eq!(cache.len(), 2);
        Ok(())
    }

    #[test]
    fn a_visible_page_evicts_margin_pages_to_fit() -> Result<(), CacheError> {
        let mut cache = PageWindowCache::new(900);
        cache.set_visible(10..12, 100);
        cache.insert(9, bitmap(10, 10))?;
        cache.insert(12, bitmap(10, 10))?;
        cache.insert(10, bitmap(10, 10))?;
        assert!(cache.bytes() <= 900);
        assert!(cache.get(10).is_some());
        assert_eq!(
            cache.len(),
            2,
            "one margin page made room for the visible page"
        );
        Ok(())
    }

    #[test]
    fn oversized_or_malformed_bitmaps_and_off_window_pages_are_refused() {
        let mut cache = PageWindowCache::new(100);
        cache.set_visible(0..1, 10);
        assert_eq!(
            cache.insert(0, bitmap(10, 10)),
            Err(CacheError::OverBudget {
                needed: 400,
                cap: 100
            })
        );
        assert_eq!(cache.refused(), 1);
        assert_eq!(
            cache.insert(5, bitmap(2, 2)),
            Err(CacheError::OutsideWindow { page: 5 })
        );
        let bad = PageBitmap {
            width: 4,
            height: 4,
            rgba: Arc::from(vec![0u8; 3]),
        };
        assert_eq!(cache.insert(0, bad), Err(CacheError::BadBitmap));
        let zero = PageBitmap {
            width: 0,
            height: 4,
            rgba: Arc::from(Vec::<u8>::new()),
        };
        assert_eq!(cache.insert(0, zero), Err(CacheError::BadBitmap));
        assert_eq!(cache.bytes(), 0);
    }

    #[test]
    fn missing_lists_visible_pages_before_margin_pages() {
        let mut cache = PageWindowCache::new(10_000);
        cache.set_visible(5..6, 20);
        assert_eq!(cache.missing(), vec![5, 4, 6]);
    }

    #[test]
    fn large_pdf_renders_only_the_window_and_supports_dark_reading() -> Result<(), PdfError> {
        let pdf = Arc::new(blank_pdf(40));
        let mut cache = PageWindowCache::new(10_000_000);
        cache.set_visible(20..21, 40);
        let cached = cache.fill_from_pdf(Arc::clone(&pdf), 100, 100, false)?;
        assert_eq!(cached, 3);
        assert_eq!(cache.len(), 3);
        let light = cache.get(20).map(|b| b.rgba[0]);
        assert_eq!(light, Some(255), "blank page renders white");

        // Already-cached pages are not rendered again; scrolling one page renders one page.
        assert_eq!(cache.fill_from_pdf(Arc::clone(&pdf), 100, 100, false)?, 0);
        cache.set_visible(21..22, 40);
        assert_eq!(cache.fill_from_pdf(Arc::clone(&pdf), 100, 100, false)?, 1);

        let mut dark_cache = PageWindowCache::new(10_000_000);
        dark_cache.set_visible(0..1, 40);
        dark_cache.fill_from_pdf(pdf, 100, 100, true)?;
        let dark = dark_cache.get(0).map(|b| b.rgba[0]);
        assert_eq!(dark, Some(26), "white paper inverts to dark slate");
        Ok(())
    }

    #[test]
    fn pdf_fill_refuses_render_sizes_that_cannot_fit_the_cap() -> Result<(), PdfError> {
        let mut cache = PageWindowCache::new(1_000);
        cache.set_visible(0..1, 5);
        let cached = cache.fill_from_pdf(Arc::new(blank_pdf(5)), 4096, 4096, false)?;
        assert_eq!(cached, 0);
        assert_eq!(cache.refused(), 1);
        assert!(cache.is_empty());
        assert!(
            cache
                .fill_from_pdf(Arc::new(b"not a pdf".to_vec()), 10, 10, false)
                .is_err()
        );
        Ok(())
    }
}
