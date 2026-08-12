//! Manual column alignment for human-readable table output. No table crate is
//! in this workspace's dependency set, and the tables `askm` prints are small
//! enough not to need one.

/// Print `headers` and `rows` as a left-aligned, space-padded table. Callers
/// should check `rows.is_empty()` themselves first and print a friendlier
/// message instead — this always prints at least the header line.
pub fn print(headers: &[&str], rows: &[Vec<String>]) {
    let widths = column_widths(headers, rows);
    let header_cells: Vec<String> = headers.iter().map(|h| h.to_string()).collect();
    println!("{}", format_row(&header_cells, &widths));
    println!(
        "{}",
        widths
            .iter()
            .map(|w| "-".repeat(*w))
            .collect::<Vec<_>>()
            .join("  ")
    );
    for row in rows {
        println!("{}", format_row(row, &widths));
    }
}

/// Render a list of names as a bounded summary, e.g. `a, b, c (+279 more)`.
/// Plugins routinely ship hundreds of skills, and printing every name turns a
/// table into an unreadable wall.
pub fn summarize_list(items: &[String], max: usize) -> String {
    if items.len() <= max {
        return items.join(", ");
    }
    let shown = items[..max].join(", ");
    format!("{shown} (+{} more)", items.len() - max)
}

/// Widths are counted in `char`s, matching how `format!`'s width specifier pads.
/// Using byte length here would misalign any row containing non-ASCII text.
fn column_widths(headers: &[&str], rows: &[Vec<String>]) -> Vec<usize> {
    let mut widths: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if let Some(w) = widths.get_mut(i) {
                *w = (*w).max(cell.chars().count());
            }
        }
    }
    widths
}

fn format_row(cells: &[String], widths: &[usize]) -> String {
    let padded: Vec<String> = cells
        .iter()
        .enumerate()
        .map(|(i, cell)| {
            let width = widths
                .get(i)
                .copied()
                .unwrap_or_else(|| cell.chars().count());
            format!("{cell:<width$}")
        })
        .collect();
    padded.join("  ").trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_widths_grow_to_fit_the_longest_cell() {
        let rows = vec![vec!["a".to_string(), "long-value".to_string()]];

        let widths = column_widths(&["COL1", "COL2"], &rows);

        assert_eq!(widths, vec![4, 10]);
    }

    #[test]
    fn format_row_pads_every_cell_and_trims_trailing_padding() {
        let row = format_row(&["ab".to_string(), "c".to_string()], &[5, 5]);

        assert_eq!(row, format!("{:<5}  {}", "ab", "c"));
    }

    #[test]
    fn column_widths_count_characters_not_bytes() {
        let rows = vec![vec!["日本語".to_string()]];

        let widths = column_widths(&["COL"], &rows);

        assert_eq!(widths, vec![3], "three chars, not nine bytes");
    }

    #[test]
    fn summarize_list_keeps_short_lists_intact() {
        let items = vec!["a".to_string(), "b".to_string()];

        assert_eq!(summarize_list(&items, 5), "a, b");
    }

    #[test]
    fn summarize_list_caps_long_lists_and_counts_the_remainder() {
        let items: Vec<String> = (0..10).map(|i| i.to_string()).collect();

        assert_eq!(summarize_list(&items, 3), "0, 1, 2 (+7 more)");
    }
}
