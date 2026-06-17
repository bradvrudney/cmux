//! Turning a [`Terminal`] grid into styled text runs for the webview.
//!
//! The UI renders each terminal row as a sequence of `<span>`s, one per run of
//! cells that share styling. Keeping the run-coalescing here (rather than one
//! span per cell) keeps the DOM small enough to re-render at interactive rates.

use cmux_term::{Cell, Color};

/// Default foreground/background are emitted as CSS variables so the active
/// theme (set on the app root) controls them; the fallbacks match the dark icon.
pub const DEFAULT_FG: &str = "var(--term-fg, #cdd6f4)";
pub const DEFAULT_BG: &str = "var(--term-bg, #1e1e2e)";

/// A maximal run of same-styled cells within a row.
#[derive(Debug, Clone, PartialEq)]
pub struct StyledRun {
    pub style: String,
    pub text: String,
}

/// A normalized text selection over a viewport, in (row, col) cell coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewportSelection {
    pub start: (usize, usize),
    pub end: (usize, usize),
}

impl ViewportSelection {
    /// Build from an anchor and the current point, normalizing reading order.
    pub fn new(a: (usize, usize), b: (usize, usize)) -> Self {
        let (start, end) = if a <= b { (a, b) } else { (b, a) };
        Self { start, end }
    }

    /// Inclusive `(lo, hi)` selected columns for `row` of `width` cells, or
    /// `None` if the row falls outside the selection.
    pub fn cols_for_row(&self, row: usize, width: usize) -> Option<(usize, usize)> {
        if width == 0 || row < self.start.0 || row > self.end.0 {
            return None;
        }
        let last = width - 1;
        let lo = if row == self.start.0 {
            self.start.1.min(last)
        } else {
            0
        };
        let hi = if row == self.end.0 {
            self.end.1.min(last)
        } else {
            last
        };
        Some((lo, hi))
    }
}

/// Coalesce a set of rows (a terminal viewport, possibly scrolled back) into
/// styled runs for the webview, tinting the cells covered by `sel` (if any).
pub fn viewport_to_runs_sel(
    rows: &[Vec<Cell>],
    sel: Option<ViewportSelection>,
) -> Vec<Vec<StyledRun>> {
    rows.iter()
        .enumerate()
        .map(|(r, row)| {
            let cols = sel.and_then(|s| s.cols_for_row(r, row.len()));
            row_to_runs(row, cols)
        })
        .collect()
}

fn row_to_runs(row: &[Cell], sel_cols: Option<(usize, usize)>) -> Vec<StyledRun> {
    let mut runs: Vec<StyledRun> = Vec::new();
    let mut current_style: Option<String> = None;
    for (i, cell) in row.iter().enumerate() {
        let mut style = cell_style(cell);
        // A selected cell appends a selection background; the later CSS
        // declaration wins, so this also overrides the cell's own background.
        if sel_cols.map_or(false, |(lo, hi)| i >= lo && i <= hi) {
            style.push_str("background-color:var(--sel-bg,#5b6078);");
        }
        let ch = if cell.c == '\0' { ' ' } else { cell.c };
        match (&current_style, runs.last_mut()) {
            (Some(s), Some(last)) if *s == style => last.text.push(ch),
            _ => {
                runs.push(StyledRun {
                    style: style.clone(),
                    text: ch.to_string(),
                });
                current_style = Some(style);
            }
        }
    }
    if runs.is_empty() {
        runs.push(StyledRun {
            style: String::new(),
            text: " ".to_string(),
        });
    }
    runs
}

fn cell_style(cell: &Cell) -> String {
    let (mut fg, mut bg) = (color_css(cell.fg, true), color_css(cell.bg, false));
    if cell.flags.inverse {
        std::mem::swap(&mut fg, &mut bg);
    }
    let mut s = format!("color:{fg};background-color:{bg};");
    if cell.flags.bold {
        s.push_str("font-weight:bold;");
    }
    if cell.flags.italic {
        s.push_str("font-style:italic;");
    }
    if cell.flags.underline {
        s.push_str("text-decoration:underline;");
    }
    s
}

/// CSS color for a terminal [`Color`]. `is_fg` picks the right default.
pub fn color_css(c: Color, is_fg: bool) -> String {
    match c {
        Color::Default => {
            if is_fg {
                DEFAULT_FG.to_string()
            } else {
                DEFAULT_BG.to_string()
            }
        }
        Color::Rgb(r, g, b) => format!("rgb({r},{g},{b})"),
        Color::Indexed(i) => {
            let (r, g, b) = xterm256(i);
            format!("rgb({r},{g},{b})")
        }
    }
}

/// The standard xterm 256-color palette → RGB.
pub fn xterm256(i: u8) -> (u8, u8, u8) {
    // 0..15: system colors.
    const SYS: [(u8, u8, u8); 16] = [
        (0, 0, 0),
        (205, 0, 0),
        (0, 205, 0),
        (205, 205, 0),
        (0, 0, 238),
        (205, 0, 205),
        (0, 205, 205),
        (229, 229, 229),
        (127, 127, 127),
        (255, 0, 0),
        (0, 255, 0),
        (255, 255, 0),
        (92, 92, 255),
        (255, 0, 255),
        (0, 255, 255),
        (255, 255, 255),
    ];
    match i {
        0..=15 => SYS[i as usize],
        16..=231 => {
            // 6x6x6 color cube.
            let i = i - 16;
            let r = i / 36;
            let g = (i % 36) / 6;
            let b = i % 6;
            let step = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            (step(r), step(g), step(b))
        }
        232..=255 => {
            // 24-step grayscale ramp.
            let v = 8 + (i - 232) * 10;
            (v, v, v)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cmux_term::Terminal;

    #[test]
    fn palette_cube_and_grayscale() {
        assert_eq!(xterm256(0), (0, 0, 0));
        assert_eq!(xterm256(15), (255, 255, 255));
        // 16 is the start of the cube = black.
        assert_eq!(xterm256(16), (0, 0, 0));
        // 21 is pure blue in the cube (r=0,g=0,b=5).
        assert_eq!(xterm256(21), (0, 0, 255));
        // grayscale ramp is monotonic.
        assert!(xterm256(232).0 < xterm256(255).0);
    }

    #[test]
    fn default_colors_use_theme_variables() {
        assert_eq!(color_css(Color::Default, true), DEFAULT_FG);
        assert_eq!(color_css(Color::Default, false), DEFAULT_BG);
        assert!(color_css(Color::Default, true).starts_with("var(--term-fg"));
        assert_eq!(color_css(Color::Rgb(1, 2, 3), true), "rgb(1,2,3)");
    }

    #[test]
    fn runs_coalesce_same_style() {
        let mut term = Terminal::new(1, 10);
        term.feed(b"hello");
        let runs = viewport_to_runs_sel(&term.viewport(0), None);
        assert_eq!(runs.len(), 1);
        // "hello" + trailing blanks: all default style, so it should coalesce
        // into a single run beginning with "hello".
        assert!(runs[0][0].text.starts_with("hello"));
    }

    #[test]
    fn selection_cols_per_row() {
        let s = ViewportSelection::new((0, 2), (2, 4));
        // First row: from anchor col to end of line.
        assert_eq!(s.cols_for_row(0, 10), Some((2, 9)));
        // Middle row: whole line.
        assert_eq!(s.cols_for_row(1, 10), Some((0, 9)));
        // Last row: start of line to active col.
        assert_eq!(s.cols_for_row(2, 10), Some((0, 4)));
        // Outside the selection.
        assert_eq!(s.cols_for_row(3, 10), None);
        // Single-row selection is the inclusive column span.
        let one = ViewportSelection::new((1, 5), (1, 2));
        assert_eq!(one.cols_for_row(1, 10), Some((2, 5)));
    }

    #[test]
    fn selection_tints_runs() {
        let mut term = Terminal::new(1, 6);
        term.feed(b"abcdef");
        let sel = ViewportSelection::new((0, 1), (0, 3));
        let runs = viewport_to_runs_sel(&term.viewport(0), Some(sel));
        let joined: String = runs[0].iter().map(|r| r.text.clone()).collect();
        assert_eq!(joined, "abcdef");
        // The selected middle cells carry the selection background.
        assert!(runs[0].iter().any(|r| r.style.contains("--sel-bg")));
        // Unselected cells exist too (the selection split the row).
        assert!(runs[0].iter().any(|r| !r.style.contains("--sel-bg")));
    }

    #[test]
    fn color_change_splits_runs() {
        let mut term = Terminal::new(1, 10);
        term.feed(b"a\x1b[31mb");
        let runs = viewport_to_runs_sel(&term.viewport(0), None);
        // 'a' default, then red 'b' (then blanks) — at least two runs.
        assert!(runs[0].len() >= 2);
        assert!(runs[0][0].text.starts_with('a'));
    }
}
