/// Number of rows before page `page` (1-based) of `page_size` rows.
pub fn page_offset(page: u32, page_size: u32) -> u64 {
    u64::from(page.saturating_sub(1)) * u64::from(page_size)
}

#[cfg(test)]
mod tests {
    use super::page_offset;

    #[test]
    fn calculates_page_offsets() {
        assert_eq!(page_offset(1, 100), 0);
        assert_eq!(page_offset(3, 50), 100);
        // Page 0 is treated as the first page.
        assert_eq!(page_offset(0, 50), 0);
    }

    #[test]
    fn does_not_overflow_on_deep_pages() {
        assert_eq!(
            page_offset(u32::MAX, u32::MAX),
            (u64::from(u32::MAX) - 1) * u64::from(u32::MAX)
        );
    }
}
