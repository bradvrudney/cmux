//! VT/ANSI terminal-emulation grid for cmux-linux panes.
//!
//! Parses a stream of terminal output bytes (via the [`vte`] parser) into a
//! renderable [`Screen`] grid of [`Cell`]s. A GUI (webview) can turn this grid
//! into styled HTML spans; tests and the control socket can use
//! [`Terminal::render_to_string`] for a plain-text snapshot.

use serde::{Deserialize, Serialize};
use vte::{Params, Parser, Perform};

/// A terminal color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Color {
    /// The terminal's default foreground/background color.
    Default,
    /// An index into the 256-color palette (0-15 basic/bright, 16-255 extended).
    Indexed(u8),
    /// A 24-bit truecolor value.
    Rgb(u8, u8, u8),
}

impl Default for Color {
    fn default() -> Self {
        Color::Default
    }
}

/// Style flags applied to a [`Cell`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CellFlags {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
}

/// A single grid cell: a character plus its colors and style flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cell {
    pub c: char,
    pub fg: Color,
    pub bg: Color,
    pub flags: CellFlags,
}

impl Default for Cell {
    fn default() -> Self {
        Cell {
            c: ' ',
            fg: Color::Default,
            bg: Color::Default,
            flags: CellFlags::default(),
        }
    }
}

impl Cell {
    /// Reset the cell to a blank space with default colors/flags.
    fn reset(&mut self) {
        *self = Cell::default();
    }
}

/// The cursor position and visibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cursor {
    pub row: usize,
    pub col: usize,
    pub visible: bool,
}

impl Default for Cursor {
    fn default() -> Self {
        Cursor {
            row: 0,
            col: 0,
            visible: true,
        }
    }
}

/// A fixed-size grid of [`Cell`]s with a cursor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Screen {
    rows: usize,
    cols: usize,
    cells: Vec<Cell>,
    cursor: Cursor,
}

impl Screen {
    /// Create a blank screen with the given dimensions (each clamped to >= 1).
    pub fn new(rows: usize, cols: usize) -> Self {
        let rows = rows.max(1);
        let cols = cols.max(1);
        Screen {
            rows,
            cols,
            cells: vec![Cell::default(); rows * cols],
            cursor: Cursor::default(),
        }
    }

    #[inline]
    pub fn rows(&self) -> usize {
        self.rows
    }

    #[inline]
    pub fn cols(&self) -> usize {
        self.cols
    }

    #[inline]
    pub fn cursor(&self) -> Cursor {
        self.cursor
    }

    #[inline]
    fn index(&self, row: usize, col: usize) -> usize {
        row * self.cols + col
    }

    /// Borrow the cell at `(row, col)`.
    ///
    /// # Panics
    /// Panics if `row >= rows()` or `col >= cols()`.
    pub fn cell(&self, row: usize, col: usize) -> &Cell {
        let i = self.index(row, col);
        &self.cells[i]
    }

    fn cell_mut(&mut self, row: usize, col: usize) -> &mut Cell {
        let i = self.index(row, col);
        &mut self.cells[i]
    }

    /// Borrow a full row of cells for rendering.
    ///
    /// # Panics
    /// Panics if `n >= rows()`.
    pub fn row(&self, n: usize) -> &[Cell] {
        let start = n * self.cols;
        &self.cells[start..start + self.cols]
    }

    fn clear_row(&mut self, row: usize) {
        let start = row * self.cols;
        for cell in &mut self.cells[start..start + self.cols] {
            cell.reset();
        }
    }

    fn clear_all(&mut self) {
        for cell in &mut self.cells {
            cell.reset();
        }
    }

    /// Scroll the whole grid up by one line; the top line is lost and a fresh
    /// blank line appears at the bottom.
    fn scroll_up(&mut self) {
        self.cells.rotate_left(self.cols);
        let last = self.rows - 1;
        self.clear_row(last);
    }

    /// Resize the grid, preserving content where it fits (top-left anchored)
    /// and clamping the cursor within the new bounds.
    fn resize(&mut self, rows: usize, cols: usize) {
        let rows = rows.max(1);
        let cols = cols.max(1);
        if rows == self.rows && cols == self.cols {
            return;
        }
        let mut new_cells = vec![Cell::default(); rows * cols];
        let copy_rows = rows.min(self.rows);
        let copy_cols = cols.min(self.cols);
        for r in 0..copy_rows {
            for c in 0..copy_cols {
                new_cells[r * cols + c] = self.cells[r * self.cols + c];
            }
        }
        self.cells = new_cells;
        self.rows = rows;
        self.cols = cols;
        if self.cursor.row >= rows {
            self.cursor.row = rows - 1;
        }
        if self.cursor.col >= cols {
            self.cursor.col = cols - 1;
        }
    }
}

/// Currently-active text attributes, applied to printed cells.
#[derive(Debug, Clone, Copy, Default)]
struct Pen {
    fg: Color,
    bg: Color,
    flags: CellFlags,
}

/// A terminal emulator: owns a [`Parser`] and a [`Screen`], advancing the
/// parser over fed bytes and mutating the screen accordingly.
pub struct Terminal {
    parser: Parser,
    state: TermState,
}

/// The mutable emulator state that implements [`vte::Perform`]. Kept separate
/// from the [`Parser`] so the borrow checker is satisfied in [`Terminal::feed`].
struct TermState {
    screen: Screen,
    pen: Pen,
    bell: bool,
}

impl Terminal {
    /// Create a terminal with a blank `rows` x `cols` screen.
    pub fn new(rows: usize, cols: usize) -> Self {
        Terminal {
            parser: Parser::new(),
            state: TermState {
                screen: Screen::new(rows, cols),
                pen: Pen::default(),
                bell: false,
            },
        }
    }

    /// Advance the parser over `bytes`, mutating the screen.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.state, bytes);
    }

    /// Resize the grid (simple clamp; no reflow). Cursor stays within bounds
    /// and content is preserved where it fits.
    pub fn resize(&mut self, rows: usize, cols: usize) {
        self.state.screen.resize(rows, cols);
    }

    #[inline]
    pub fn rows(&self) -> usize {
        self.state.screen.rows()
    }

    #[inline]
    pub fn cols(&self) -> usize {
        self.state.screen.cols()
    }

    #[inline]
    pub fn cursor(&self) -> Cursor {
        self.state.screen.cursor()
    }

    /// Borrow the cell at `(row, col)`.
    #[inline]
    pub fn cell(&self, row: usize, col: usize) -> &Cell {
        self.state.screen.cell(row, col)
    }

    /// Borrow a full row of cells for rendering.
    #[inline]
    pub fn row(&self, n: usize) -> &[Cell] {
        self.state.screen.row(n)
    }

    /// Borrow the underlying screen.
    #[inline]
    pub fn screen(&self) -> &Screen {
        &self.state.screen
    }

    /// Whether a BEL (`\x07`) has been seen since the last [`Terminal::take_bell`].
    #[inline]
    pub fn bell_rang(&self) -> bool {
        self.state.bell
    }

    /// Return whether a BEL has rung and reset the flag.
    #[inline]
    pub fn take_bell(&mut self) -> bool {
        let rang = self.state.bell;
        self.state.bell = false;
        rang
    }

    /// Render the whole screen to a plain `String`: one `\n` per row, with
    /// trailing spaces trimmed from each line.
    pub fn render_to_string(&self) -> String {
        let screen = &self.state.screen;
        let mut out = String::new();
        for r in 0..screen.rows() {
            let mut line = String::with_capacity(screen.cols());
            for c in 0..screen.cols() {
                line.push(screen.cell(r, c).c);
            }
            out.push_str(line.trim_end_matches(' '));
            if r + 1 < screen.rows() {
                out.push('\n');
            }
        }
        out
    }
}

impl TermState {
    /// Move the cursor to the next line, scrolling if already on the last row.
    fn line_feed(&mut self) {
        let s = &mut self.screen;
        if s.cursor.row + 1 >= s.rows {
            s.scroll_up();
        } else {
            s.cursor.row += 1;
        }
    }

    /// Write a character at the cursor and advance, wrapping at end of row.
    fn write_char(&mut self, c: char) {
        let (fg, bg, flags) = (self.pen.fg, self.pen.bg, self.pen.flags);
        let s = &mut self.screen;
        // If past the last column, wrap to the start of the next line first.
        if s.cursor.col >= s.cols {
            s.cursor.col = 0;
            if s.cursor.row + 1 >= s.rows {
                s.scroll_up();
            } else {
                s.cursor.row += 1;
            }
        }
        let (row, col) = (s.cursor.row, s.cursor.col);
        let cell = s.cell_mut(row, col);
        cell.c = c;
        cell.fg = fg;
        cell.bg = bg;
        cell.flags = flags;
        s.cursor.col += 1;
    }

    fn apply_sgr(&mut self, params: &Params) {
        // Flatten params and subparams into a single sequence so we handle both
        // `38;5;n` (separate params) and `38:5:n` (subparams) uniformly.
        let mut flat: Vec<u16> = Vec::new();
        for p in params.iter() {
            flat.extend_from_slice(p);
        }
        if flat.is_empty() {
            self.pen = Pen::default();
            return;
        }
        let mut i = 0;
        while i < flat.len() {
            let code = flat[i];
            match code {
                0 => self.pen = Pen::default(),
                1 => self.pen.flags.bold = true,
                3 => self.pen.flags.italic = true,
                4 => self.pen.flags.underline = true,
                7 => self.pen.flags.inverse = true,
                22 => self.pen.flags.bold = false,
                23 => self.pen.flags.italic = false,
                24 => self.pen.flags.underline = false,
                27 => self.pen.flags.inverse = false,
                30..=37 => self.pen.fg = Color::Indexed((code - 30) as u8),
                39 => self.pen.fg = Color::Default,
                40..=47 => self.pen.bg = Color::Indexed((code - 40) as u8),
                49 => self.pen.bg = Color::Default,
                90..=97 => self.pen.fg = Color::Indexed((code - 90 + 8) as u8),
                100..=107 => self.pen.bg = Color::Indexed((code - 100 + 8) as u8),
                38 | 48 => {
                    let is_fg = code == 38;
                    match flat.get(i + 1).copied() {
                        Some(5) => {
                            if let Some(&n) = flat.get(i + 2) {
                                let color = Color::Indexed(n as u8);
                                if is_fg {
                                    self.pen.fg = color;
                                } else {
                                    self.pen.bg = color;
                                }
                                i += 2;
                            }
                        }
                        Some(2) => {
                            if let (Some(&r), Some(&g), Some(&b)) =
                                (flat.get(i + 2), flat.get(i + 3), flat.get(i + 4))
                            {
                                let color = Color::Rgb(r as u8, g as u8, b as u8);
                                if is_fg {
                                    self.pen.fg = color;
                                } else {
                                    self.pen.bg = color;
                                }
                                i += 4;
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }

    fn erase_display(&mut self, mode: u16) {
        let (rows, cols) = (self.screen.rows, self.screen.cols);
        let (cr, cc) = (self.screen.cursor.row, self.screen.cursor.col);
        match mode {
            0 => {
                for c in cc.min(cols - 1)..cols {
                    self.screen.cell_mut(cr, c).reset();
                }
                for r in (cr + 1)..rows {
                    self.screen.clear_row(r);
                }
            }
            1 => {
                for r in 0..cr {
                    self.screen.clear_row(r);
                }
                for c in 0..=cc.min(cols - 1) {
                    self.screen.cell_mut(cr, c).reset();
                }
            }
            _ => self.screen.clear_all(), // 2 and 3
        }
    }

    fn erase_line(&mut self, mode: u16) {
        let cols = self.screen.cols;
        let (cr, cc) = (self.screen.cursor.row, self.screen.cursor.col);
        match mode {
            0 => {
                for c in cc.min(cols - 1)..cols {
                    self.screen.cell_mut(cr, c).reset();
                }
            }
            1 => {
                for c in 0..=cc.min(cols - 1) {
                    self.screen.cell_mut(cr, c).reset();
                }
            }
            _ => self.screen.clear_row(cr), // 2
        }
    }
}

/// Read the first param as a 1-based count, mapping absent/zero to `default`.
fn param_or(params: &Params, default: u16) -> u16 {
    match params.iter().next().and_then(|p| p.first().copied()) {
        Some(0) | None => default,
        Some(v) => v,
    }
}

/// Read the Nth param's primary value, or `None` if absent/zero.
fn nth_param(params: &Params, n: usize) -> Option<u16> {
    params
        .iter()
        .nth(n)
        .and_then(|p| p.first().copied())
        .filter(|&v| v != 0)
}

/// Read the first param verbatim (zero allowed), defaulting to 0 if absent.
fn first_param_raw(params: &Params) -> u16 {
    params
        .iter()
        .next()
        .and_then(|p| p.first().copied())
        .unwrap_or(0)
}

impl Perform for TermState {
    fn print(&mut self, c: char) {
        self.write_char(c);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' | 0x0b | 0x0c => self.line_feed(), // LF, VT, FF -> line down
            b'\r' => self.screen.cursor.col = 0,
            b'\t' => {
                let s = &mut self.screen;
                let next = ((s.cursor.col / 8) + 1) * 8;
                s.cursor.col = next.min(s.cols - 1);
            }
            0x08 => {
                let s = &mut self.screen;
                if s.cursor.col > 0 {
                    s.cursor.col -= 1;
                }
            }
            0x07 => self.bell = true, // BEL
            _ => {}
        }
    }

    fn csi_dispatch(
        &mut self,
        params: &Params,
        intermediates: &[u8],
        _ignore: bool,
        action: char,
    ) {
        match action {
            'A' => {
                let n = param_or(params, 1) as usize;
                let s = &mut self.screen;
                s.cursor.row = s.cursor.row.saturating_sub(n);
            }
            'B' => {
                let n = param_or(params, 1) as usize;
                let s = &mut self.screen;
                s.cursor.row = (s.cursor.row + n).min(s.rows - 1);
            }
            'C' => {
                let n = param_or(params, 1) as usize;
                let s = &mut self.screen;
                s.cursor.col = (s.cursor.col + n).min(s.cols - 1);
            }
            'D' => {
                let n = param_or(params, 1) as usize;
                let s = &mut self.screen;
                s.cursor.col = s.cursor.col.saturating_sub(n);
            }
            'H' | 'f' => {
                // CUP / HVP: 1-based row;col -> 0-based.
                let row = nth_param(params, 0).unwrap_or(1) as usize - 1;
                let col = nth_param(params, 1).unwrap_or(1) as usize - 1;
                let s = &mut self.screen;
                s.cursor.row = row.min(s.rows - 1);
                s.cursor.col = col.min(s.cols - 1);
            }
            'J' => self.erase_display(first_param_raw(params)),
            'K' => self.erase_line(first_param_raw(params)),
            'm' => self.apply_sgr(params),
            'h' | 'l' if intermediates.first() == Some(&b'?') => {
                // DEC private mode set/reset. Handle DECTCEM (?25) visibility.
                let show = action == 'h';
                for p in params.iter() {
                    if p.first() == Some(&25) {
                        self.screen.cursor.visible = show;
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn print_hello_advances_cursor() {
        let mut t = Terminal::new(24, 80);
        t.feed(b"hello");
        assert_eq!(t.cell(0, 0).c, 'h');
        assert_eq!(t.cell(0, 1).c, 'e');
        assert_eq!(t.cell(0, 2).c, 'l');
        assert_eq!(t.cell(0, 3).c, 'l');
        assert_eq!(t.cell(0, 4).c, 'o');
        assert_eq!(t.cursor().col, 5);
        assert_eq!(t.cursor().row, 0);
    }

    #[test]
    fn crlf_moves_to_next_row() {
        let mut t = Terminal::new(24, 80);
        t.feed(b"a\r\nb");
        assert_eq!(t.cell(0, 0).c, 'a');
        assert_eq!(t.cell(1, 0).c, 'b');
        assert_eq!(t.cursor().row, 1);
        assert_eq!(t.cursor().col, 1);
    }

    #[test]
    fn sgr_red_foreground() {
        let mut t = Terminal::new(24, 80);
        t.feed(b"\x1b[31mX");
        assert_eq!(t.cell(0, 0).c, 'X');
        assert_eq!(t.cell(0, 0).fg, Color::Indexed(1));
    }

    #[test]
    fn sgr_bright_bold_256_and_rgb() {
        let mut t = Terminal::new(24, 80);
        // bold (1) + bright green fg (92 -> index 10)
        t.feed(b"\x1b[1;92mA");
        assert_eq!(t.cell(0, 0).fg, Color::Indexed(10));
        assert!(t.cell(0, 0).flags.bold);
        // reset, then 256-color fg index 200
        t.feed(b"\x1b[0m\x1b[38;5;200mB");
        assert_eq!(t.cell(0, 1).fg, Color::Indexed(200));
        assert!(!t.cell(0, 1).flags.bold);
        // truecolor bg
        t.feed(b"\x1b[48;2;10;20;30mC");
        assert_eq!(t.cell(0, 2).bg, Color::Rgb(10, 20, 30));
        // colon-subparam form 38:5:99
        t.feed(b"\x1b[0m\x1b[38:5:99mD");
        assert_eq!(t.cell(0, 3).fg, Color::Indexed(99));
    }

    #[test]
    fn cup_positions_cursor() {
        let mut t = Terminal::new(24, 80);
        t.feed(b"\x1b[3;5HZ");
        assert_eq!(t.cell(2, 4).c, 'Z');
    }

    #[test]
    fn ed_2_clears_screen() {
        let mut t = Terminal::new(5, 10);
        t.feed(b"hello\r\nworld");
        t.feed(b"\x1b[2J");
        for r in 0..t.rows() {
            for c in 0..t.cols() {
                assert_eq!(t.cell(r, c).c, ' ');
            }
        }
    }

    #[test]
    fn el_erase_start_to_cursor() {
        let mut t = Terminal::new(3, 10);
        t.feed(b"abcdef");
        t.feed(b"\x1b[1;4H"); // row 0, col 3
        t.feed(b"\x1b[1K"); // erase cols 0..=3
        assert_eq!(t.cell(0, 0).c, ' ');
        assert_eq!(t.cell(0, 3).c, ' ');
        assert_eq!(t.cell(0, 4).c, 'e');
    }

    #[test]
    fn bell_flag_set_and_reset() {
        let mut t = Terminal::new(4, 4);
        assert!(!t.bell_rang());
        t.feed(b"\x07");
        assert!(t.bell_rang());
        assert!(t.take_bell());
        assert!(!t.bell_rang());
        assert!(!t.take_bell());
    }

    #[test]
    fn scroll_on_overflow() {
        let mut t = Terminal::new(2, 5);
        t.feed(b"one\r\ntwo\r\nthree");
        // After 3 lines on a 2-row grid, "one" scrolled off.
        assert_eq!(t.cell(0, 0).c, 't');
        assert_eq!(t.cell(0, 1).c, 'w');
        assert_eq!(t.cell(0, 2).c, 'o');
        assert_eq!(t.cell(1, 0).c, 't');
        assert_eq!(t.cell(1, 4).c, 'e');
    }

    #[test]
    fn render_to_string_trims_and_joins() {
        let mut t = Terminal::new(3, 10);
        t.feed(b"hi\r\nthere");
        assert_eq!(t.render_to_string(), "hi\nthere\n");
    }

    #[test]
    fn backspace_and_tab() {
        let mut t = Terminal::new(2, 20);
        t.feed(b"ab\x08");
        assert_eq!(t.cursor().col, 1);
        t.feed(b"\x1b[1;1H\t");
        assert_eq!(t.cursor().col, 8);
    }

    #[test]
    fn cursor_movement_csi() {
        let mut t = Terminal::new(10, 10);
        t.feed(b"\x1b[5;5H"); // row 4 col 4
        t.feed(b"\x1b[2A"); // up 2 -> row 2
        assert_eq!(t.cursor().row, 2);
        t.feed(b"\x1b[3B"); // down 3 -> row 5
        assert_eq!(t.cursor().row, 5);
        t.feed(b"\x1b[2C"); // forward 2 -> col 6
        assert_eq!(t.cursor().col, 6);
        t.feed(b"\x1b[4D"); // back 4 -> col 2
        assert_eq!(t.cursor().col, 2);
    }

    #[test]
    fn resize_preserves_content_and_clamps_cursor() {
        let mut t = Terminal::new(5, 10);
        t.feed(b"hello");
        t.resize(3, 4);
        assert_eq!(t.rows(), 3);
        assert_eq!(t.cols(), 4);
        assert_eq!(t.cell(0, 0).c, 'h');
        assert_eq!(t.cell(0, 3).c, 'l');
        // cursor col was 5, clamped to 3.
        assert_eq!(t.cursor().col, 3);
    }

    #[test]
    fn wrap_at_end_of_row() {
        let mut t = Terminal::new(3, 3);
        t.feed(b"abcd");
        assert_eq!(t.cell(0, 0).c, 'a');
        assert_eq!(t.cell(0, 2).c, 'c');
        assert_eq!(t.cell(1, 0).c, 'd');
        assert_eq!(t.cursor().row, 1);
        assert_eq!(t.cursor().col, 1);
    }

    #[test]
    fn cursor_visibility_toggle() {
        let mut t = Terminal::new(4, 4);
        assert!(t.cursor().visible);
        t.feed(b"\x1b[?25l");
        assert!(!t.cursor().visible);
        t.feed(b"\x1b[?25h");
        assert!(t.cursor().visible);
    }

    #[test]
    fn row_accessor_matches_cells() {
        let mut t = Terminal::new(2, 4);
        t.feed(b"hi");
        let r0 = t.row(0);
        assert_eq!(r0.len(), 4);
        assert_eq!(r0[0].c, 'h');
        assert_eq!(r0[1].c, 'i');
        assert_eq!(r0[2].c, ' ');
    }
}
