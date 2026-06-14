//! Turning a [`Terminal`] grid into styled text runs for the webview.
//!
//! The UI renders each terminal row as a sequence of `<span>`s, one per run of
//! cells that share styling. Keeping the run-coalescing here (rather than one
//! span per cell) keeps the DOM small enough to re-render at interactive rates.

use cmux_term::{Cell, Color, Terminal};

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

/// Coalesce each terminal row into styled runs.
pub fn rows_to_runs(term: &Terminal) -> Vec<Vec<StyledRun>> {
    (0..term.rows())
        .map(|r| row_to_runs(term.row(r)))
        .collect()
}

fn row_to_runs(row: &[Cell]) -> Vec<StyledRun> {
    let mut runs: Vec<StyledRun> = Vec::new();
    let mut current_style: Option<String> = None;
    for cell in row {
        let style = cell_style(cell);
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
        let runs = rows_to_runs(&term);
        assert_eq!(runs.len(), 1);
        // "hello" + trailing blanks: all default style, so it should coalesce
        // into a single run beginning with "hello".
        assert!(runs[0][0].text.starts_with("hello"));
    }

    #[test]
    fn color_change_splits_runs() {
        let mut term = Terminal::new(1, 10);
        term.feed(b"a\x1b[31mb");
        let runs = rows_to_runs(&term);
        // 'a' default, then red 'b' (then blanks) — at least two runs.
        assert!(runs[0].len() >= 2);
        assert!(runs[0][0].text.starts_with('a'));
    }
}
