//! Splitting long lists into pages. Pure, shared by the CLI and the TUI.

/// One page of a list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page<'a, T> {
    /// The items on this page.
    pub items: &'a [T],
    /// This page, counting from 1.
    pub page: usize,
    /// Number of pages (at least 1, even for an empty list).
    pub pages: usize,
    /// Items per page.
    pub per_page: usize,
    /// Items in the whole list.
    pub total: usize,
    /// Position of the first item on this page, counting from 1 (0 if empty).
    pub first: usize,
    /// Position of the last item on this page (0 if empty).
    pub last: usize,
}

impl<T> Page<'_, T> {
    /// True when there is a page after this one.
    pub fn has_next(&self) -> bool {
        self.page < self.pages
    }

    /// "page 2 of 7 · 21–40 of 134", or "nothing yet" when empty.
    pub fn summary(&self) -> String {
        if self.total == 0 {
            return "nothing yet".into();
        }
        format!(
            "page {} of {} · {}–{} of {}",
            self.page, self.pages, self.first, self.last, self.total
        )
    }
}

/// Number of pages for `total` items.
pub fn page_count(total: usize, per_page: usize) -> usize {
    total.div_ceil(per_page.max(1)).max(1)
}

/// Page `page` (from 1, clamped to the valid range) of `items`.
pub fn paginate<T>(items: &[T], page: usize, per_page: usize) -> Page<'_, T> {
    let per_page = per_page.max(1);
    let pages = page_count(items.len(), per_page);
    let page = page.clamp(1, pages);
    let start = ((page - 1) * per_page).min(items.len());
    let end = (start + per_page).min(items.len());
    Page {
        items: &items[start..end],
        page,
        pages,
        per_page,
        total: items.len(),
        first: if end > start { start + 1 } else { 0 },
        last: end,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_split_and_clamp() {
        let items: Vec<u32> = (1..=45).collect();
        let p = paginate(&items, 1, 20);
        assert_eq!(
            (p.items.len(), p.page, p.pages, p.first, p.last),
            (20, 1, 3, 1, 20)
        );
        let last = paginate(&items, 3, 20);
        assert_eq!(last.items, &[41, 42, 43, 44, 45]);
        assert_eq!((last.first, last.last), (41, 45));
        assert!(!last.has_next());
        assert_eq!(paginate(&items, 99, 20).page, 3, "past the end clamps");
        assert_eq!(paginate(&items, 0, 20).page, 1, "before the start clamps");
        assert_eq!(p.summary(), "page 1 of 3 · 1–20 of 45");
    }

    #[test]
    fn empty_and_odd_sizes() {
        let empty: Vec<u32> = Vec::new();
        let p = paginate(&empty, 1, 20);
        assert_eq!((p.pages, p.first, p.last, p.total), (1, 0, 0, 0));
        assert_eq!(p.summary(), "nothing yet");
        let one = [7u32];
        assert_eq!(
            paginate(&one, 1, 0).per_page,
            1,
            "zero per page is treated as one"
        );
        assert_eq!(page_count(20, 20), 1);
        assert_eq!(page_count(21, 20), 2);
    }
}
