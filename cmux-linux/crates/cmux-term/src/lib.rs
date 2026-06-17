//! VT/ANSI terminal-emulation grid for cmux-linux panes.
//!
//! Parses a stream of terminal output bytes (via the [`vte`] parser) into a
//! renderable [`Screen`] grid of [`Cell`]s. A GUI (webview) can turn this grid
//! into styled HTML spans; tests and the control socket can use
//! [`Terminal::render_to_string`] for a plain-text snapshot.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};
use unicode_width::UnicodeWidthChar;
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

    /// Scroll the inclusive row range `[top, bottom]` up by `n` lines. Lines at
    /// the top of the region are lost; `n` blank lines appear at the bottom.
    fn scroll_region_up(&mut self, top: usize, bottom: usize, n: usize) {
        if top > bottom || bottom >= self.rows {
            return;
        }
        let region_rows = bottom - top + 1;
        let n = n.min(region_rows);
        for r in top..=bottom {
            let src = r + n;
            if src <= bottom {
                for c in 0..self.cols {
                    let v = self.cells[src * self.cols + c];
                    self.cells[r * self.cols + c] = v;
                }
            } else {
                self.clear_row(r);
            }
        }
    }

    /// Scroll the inclusive row range `[top, bottom]` down by `n` lines. Lines at
    /// the bottom of the region are lost; `n` blank lines appear at the top.
    fn scroll_region_down(&mut self, top: usize, bottom: usize, n: usize) {
        if top > bottom || bottom >= self.rows {
            return;
        }
        let region_rows = bottom - top + 1;
        let n = n.min(region_rows);
        for r in (top..=bottom).rev() {
            if r >= top + n {
                let src = r - n;
                for c in 0..self.cols {
                    let v = self.cells[src * self.cols + c];
                    self.cells[r * self.cols + c] = v;
                }
            } else {
                self.clear_row(r);
            }
        }
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

/// A search hit: a line (absolute, 0 = oldest scrollback) and char column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Match {
    pub line: usize,
    pub col: usize,
}

/// All starting char-indices where `needle` occurs in `haystack`.
fn find_all(haystack: &[char], needle: &[char]) -> Vec<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return Vec::new();
    }
    (0..=haystack.len() - needle.len())
        .filter(|&start| haystack[start..start + needle.len()] == needle[..])
        .collect()
}

/// A saved cursor position (DECSC / `ESC [ s`).
#[derive(Debug, Clone, Copy, Default)]
struct SavedCursor {
    row: usize,
    col: usize,
}

/// The mutable emulator state that implements [`vte::Perform`]. Kept separate
/// from the [`Parser`] so the borrow checker is satisfied in [`Terminal::feed`].
struct TermState {
    screen: Screen,
    pen: Pen,
    bell: bool,
    /// Top scroll margin (0-based, inclusive).
    scroll_top: usize,
    /// Bottom scroll margin (0-based, inclusive).
    scroll_bottom: usize,
    /// Saved cursor for DECSC/DECRC and CSI s/u.
    saved_cursor: SavedCursor,
    /// Whether autowrap (DECAWM, `?7`) is enabled. Default on.
    autowrap: bool,
    /// Deferred-wrap latch: set after printing into the last column. The next
    /// printable char wraps to the next line before being written.
    pending_wrap: bool,
    /// Set while the alternate screen buffer is active.
    alt_active: bool,
    /// Saved primary screen while the alt buffer is active.
    saved_primary: Option<Screen>,
    /// Saved cursor/pen for the `?1049` alt-screen enter (restored on leave).
    saved_alt_cursor: SavedCursor,
    /// Latest window title from OSC 0/1/2 (drained by the host).
    title: Option<String>,
    /// Latest working directory from OSC 7 (drained by the host).
    cwd: Option<String>,
    /// Notifications raised via OSC 9 / OSC 777 (drained by the host).
    notifications: Vec<(String, String)>,
    /// Lines that scrolled off the top of the primary screen, oldest first.
    scrollback: VecDeque<Vec<Cell>>,
}

impl Terminal {
    /// Create a terminal with a blank `rows` x `cols` screen.
    pub fn new(rows: usize, cols: usize) -> Self {
        let screen = Screen::new(rows, cols);
        let bottom = screen.rows() - 1;
        Terminal {
            parser: Parser::new(),
            state: TermState {
                screen,
                pen: Pen::default(),
                bell: false,
                scroll_top: 0,
                scroll_bottom: bottom,
                saved_cursor: SavedCursor::default(),
                autowrap: true,
                pending_wrap: false,
                alt_active: false,
                saved_primary: None,
                saved_alt_cursor: SavedCursor::default(),
                title: None,
                cwd: None,
                notifications: Vec::new(),
                scrollback: VecDeque::new(),
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
        if let Some(primary) = self.state.saved_primary.as_mut() {
            primary.resize(rows, cols);
        }
        // Reset the scroll region to the full (new) screen and clear any
        // deferred wrap so it can't reference a column past the new bounds.
        self.state.scroll_top = 0;
        self.state.scroll_bottom = self.state.screen.rows() - 1;
        self.state.pending_wrap = false;
    }

    /// Whether the alternate screen buffer is currently active (vim/less/htop).
    #[inline]
    pub fn is_alt_screen(&self) -> bool {
        self.state.alt_active
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

    /// The current window title (OSC 0/1/2), if one has been set.
    pub fn title(&self) -> Option<&str> {
        self.state.title.as_deref()
    }

    /// Take and clear the latest title set since the last call.
    pub fn take_title(&mut self) -> Option<String> {
        self.state.title.take()
    }

    /// Take and clear the latest working directory set via OSC 7.
    pub fn take_cwd(&mut self) -> Option<String> {
        self.state.cwd.take()
    }

    /// Take and clear notifications raised via OSC 9 / OSC 777 (title, body).
    pub fn take_notifications(&mut self) -> Vec<(String, String)> {
        std::mem::take(&mut self.state.notifications)
    }

    /// Number of lines preserved in scrollback (history above the screen).
    pub fn scrollback_len(&self) -> usize {
        self.state.scrollback.len()
    }

    /// Total addressable lines: scrollback followed by the live screen rows.
    pub fn total_lines(&self) -> usize {
        self.state.scrollback.len() + self.state.screen.rows()
    }

    /// Case-insensitive search across scrollback + screen. Returns matches in
    /// absolute line order (line 0 = oldest scrollback line, then screen rows).
    pub fn search(&self, query: &str) -> Vec<Match> {
        if query.is_empty() {
            return Vec::new();
        }
        let needle: Vec<char> = query.to_lowercase().chars().collect();
        let mut out = Vec::new();
        let push_line = |line: usize, text: &str, out: &mut Vec<Match>| {
            let hay: Vec<char> = text.to_lowercase().chars().collect();
            for col in find_all(&hay, &needle) {
                out.push(Match { line, col });
            }
        };
        for (i, line) in self.state.scrollback.iter().enumerate() {
            let text: String = line.iter().map(|c| c.c).collect();
            push_line(i, &text, &mut out);
        }
        let base = self.state.scrollback.len();
        for r in 0..self.state.screen.rows() {
            let text: String = self.state.screen.row(r).iter().map(|c| c.c).collect();
            push_line(base + r, &text, &mut out);
        }
        out
    }

    /// The visible viewport scrolled back by `offset` lines (0 = live screen).
    /// Returns exactly `rows()` rows of cells, drawing from scrollback for the
    /// portion above the current screen. `offset` is clamped to scrollback len.
    pub fn viewport(&self, offset: usize) -> Vec<Vec<Cell>> {
        let rows = self.state.screen.rows();
        let cols = self.state.screen.cols();
        let offset = offset.min(self.state.scrollback.len());
        if offset == 0 {
            return (0..rows).map(|r| self.state.screen.row(r).to_vec()).collect();
        }
        // Build a combined view: [scrollback ... | screen ...], then take the
        // window ending `offset` lines from the bottom.
        let sb = &self.state.scrollback;
        let total = sb.len() + rows;
        let end = total - offset; // exclusive index of the window's bottom
        let start = end.saturating_sub(rows);
        let blank = vec![Cell::default(); cols];
        (start..end)
            .map(|i| {
                if i < sb.len() {
                    let mut line = sb[i].clone();
                    line.resize(cols, Cell::default());
                    line
                } else {
                    self.state.screen.row(i - sb.len()).to_vec()
                }
            })
            .chain(std::iter::repeat(blank.clone()))
            .take(rows)
            .collect()
    }

    /// Render the whole screen to a plain `String`: one `\n` per row, with
    /// trailing spaces trimmed from each line.
    pub fn render_to_string(&self) -> String {
        let screen = &self.state.screen;
        let mut out = String::new();
        for r in 0..screen.rows() {
            let mut line = String::with_capacity(screen.cols());
            let mut c = 0;
            while c < screen.cols() {
                let ch = screen.cell(r, c).c;
                line.push(ch);
                // A double-width glyph owns the following spacer cell; skip it
                // so the text isn't padded with a stray space.
                if UnicodeWidthChar::width(ch) == Some(2) {
                    c += 2;
                } else {
                    c += 1;
                }
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
    /// Index (line down). Move the cursor down one line; if at the bottom
    /// margin, scroll the scroll region up by one. Honors the scroll region.
    fn line_feed(&mut self) {
        let top = self.scroll_top;
        let bottom = self.scroll_bottom;
        if self.screen.cursor.row == bottom {
            // A line scrolling off the top of the primary screen (full-height
            // region) is preserved in scrollback for history.
            if !self.alt_active && top == 0 {
                let line: Vec<Cell> =
                    (0..self.screen.cols).map(|c| *self.screen.cell(0, c)).collect();
                self.push_scrollback(line);
            }
            self.screen.scroll_region_up(top, bottom, 1);
        } else if self.screen.cursor.row + 1 < self.screen.rows {
            self.screen.cursor.row += 1;
        }
    }

    fn push_scrollback(&mut self, line: Vec<Cell>) {
        const MAX_SCROLLBACK: usize = 10_000;
        self.scrollback.push_back(line);
        while self.scrollback.len() > MAX_SCROLLBACK {
            self.scrollback.pop_front();
        }
    }

    /// Reverse Index (RI, `ESC M`). Move the cursor up one line; if at the top
    /// margin, scroll the scroll region down by one.
    fn reverse_index(&mut self) {
        let top = self.scroll_top;
        let bottom = self.scroll_bottom;
        let s = &mut self.screen;
        if s.cursor.row == top {
            s.scroll_region_down(top, bottom, 1);
        } else if s.cursor.row > 0 {
            s.cursor.row -= 1;
        }
    }

    /// Write a character at the cursor and advance, with deferred (pending)
    /// autowrap at the end of a row.
    fn write_char(&mut self, c: char) {
        // Combining / zero-width marks: skip rather than risk corrupting the
        // grid (full combining support is a future item).
        let width = UnicodeWidthChar::width(c).unwrap_or(0);
        if width == 0 {
            return;
        }
        let (fg, bg, flags) = (self.pen.fg, self.pen.bg, self.pen.flags);

        // Resolve any deferred wrap from the previous print before writing.
        if self.pending_wrap {
            self.pending_wrap = false;
            if self.autowrap {
                self.screen.cursor.col = 0;
                self.line_feed();
            }
        }

        let cols = self.screen.cols;
        // A double-width glyph that can't fit before the right edge wraps first.
        if width == 2 && self.autowrap && self.screen.cursor.col + 1 >= cols {
            self.screen.cursor.col = 0;
            self.line_feed();
        }

        // Without autowrap, printing in the last column overwrites in place.
        let col = self.screen.cursor.col.min(cols - 1);
        let row = self.screen.cursor.row;
        {
            let cell = self.screen.cell_mut(row, col);
            cell.c = c;
            cell.fg = fg;
            cell.bg = bg;
            cell.flags = flags;
        }
        // Clear the spacer cell a wide glyph occupies.
        if width == 2 && col + 1 < cols {
            let cell = self.screen.cell_mut(row, col + 1);
            cell.c = ' ';
            cell.fg = fg;
            cell.bg = bg;
            cell.flags = flags;
        }

        let new_col = col + width;
        if new_col >= cols {
            // Past the last column: latch a pending wrap (autowrap) or stay put.
            self.screen.cursor.col = cols - 1;
            if self.autowrap {
                self.pending_wrap = true;
            }
        } else {
            self.screen.cursor.col = new_col;
        }
    }

    /// Set the scroll region from 1-based, inclusive params (DECSTBM). Empty
    /// params reset to the full screen. Moves the cursor to the region home.
    fn set_scroll_region(&mut self, top: u16, bottom: u16) {
        let rows = self.screen.rows;
        let (mut t, mut b) = if top == 0 && bottom == 0 {
            (0, rows - 1)
        } else {
            let t = (top.max(1) as usize - 1).min(rows - 1);
            let b = if bottom == 0 {
                rows - 1
            } else {
                (bottom as usize - 1).min(rows - 1)
            };
            (t, b)
        };
        if t >= b {
            // Invalid region: reset to full screen.
            t = 0;
            b = rows - 1;
        }
        self.scroll_top = t;
        self.scroll_bottom = b;
        // DECSTBM homes the cursor.
        self.screen.cursor.row = 0;
        self.screen.cursor.col = 0;
        self.pending_wrap = false;
    }

    /// Insert `n` blank lines at the cursor row, within the scroll region (IL).
    fn insert_lines(&mut self, n: usize) {
        let row = self.screen.cursor.row;
        if row < self.scroll_top || row > self.scroll_bottom {
            return;
        }
        let bottom = self.scroll_bottom;
        self.screen.scroll_region_down(row, bottom, n);
        self.pending_wrap = false;
    }

    /// Delete `n` lines at the cursor row, within the scroll region (DL).
    fn delete_lines(&mut self, n: usize) {
        let row = self.screen.cursor.row;
        if row < self.scroll_top || row > self.scroll_bottom {
            return;
        }
        let bottom = self.scroll_bottom;
        self.screen.scroll_region_up(row, bottom, n);
        self.pending_wrap = false;
    }

    /// Insert `n` blank cells at the cursor, shifting the rest of the line right
    /// (ICH). Cells shifted past the end are lost.
    fn insert_chars(&mut self, n: usize) {
        let cols = self.screen.cols;
        let row = self.screen.cursor.row;
        let col = self.screen.cursor.col.min(cols - 1);
        let n = n.min(cols - col);
        for c in (col..cols).rev() {
            if c >= col + n {
                let v = *self.screen.cell(row, c - n);
                *self.screen.cell_mut(row, c) = v;
            } else {
                self.screen.cell_mut(row, c).reset();
            }
        }
        self.pending_wrap = false;
    }

    /// Delete `n` cells at the cursor, shifting the rest of the line left (DCH).
    /// Blanks fill in at the end of the line.
    fn delete_chars(&mut self, n: usize) {
        let cols = self.screen.cols;
        let row = self.screen.cursor.row;
        let col = self.screen.cursor.col.min(cols - 1);
        let n = n.min(cols - col);
        for c in col..cols {
            if c + n < cols {
                let v = *self.screen.cell(row, c + n);
                *self.screen.cell_mut(row, c) = v;
            } else {
                self.screen.cell_mut(row, c).reset();
            }
        }
        self.pending_wrap = false;
    }

    /// Erase `n` cells at the cursor in place, without shifting (ECH).
    fn erase_chars(&mut self, n: usize) {
        let cols = self.screen.cols;
        let row = self.screen.cursor.row;
        let col = self.screen.cursor.col.min(cols - 1);
        let end = (col + n).min(cols);
        for c in col..end {
            self.screen.cell_mut(row, c).reset();
        }
        self.pending_wrap = false;
    }

    /// Enter the alternate screen buffer. `save_cursor` corresponds to `?1049`
    /// (vs `?47`/`?1047` which do not save the cursor).
    fn enter_alt_screen(&mut self, save_cursor: bool) {
        if self.alt_active {
            return;
        }
        if save_cursor {
            self.saved_alt_cursor = SavedCursor {
                row: self.screen.cursor.row,
                col: self.screen.cursor.col,
            };
        }
        let (rows, cols) = (self.screen.rows, self.screen.cols);
        let mut primary = Screen::new(rows, cols);
        std::mem::swap(&mut primary, &mut self.screen);
        self.saved_primary = Some(primary);
        self.alt_active = true;
        self.pending_wrap = false;
    }

    /// Leave the alternate screen buffer, restoring the primary buffer.
    /// `restore_cursor` corresponds to `?1049`.
    fn leave_alt_screen(&mut self, restore_cursor: bool) {
        if !self.alt_active {
            return;
        }
        if let Some(primary) = self.saved_primary.take() {
            self.screen = primary;
        }
        if restore_cursor {
            let rows = self.screen.rows;
            let cols = self.screen.cols;
            self.screen.cursor.row = self.saved_alt_cursor.row.min(rows - 1);
            self.screen.cursor.col = self.saved_alt_cursor.col.min(cols - 1);
        }
        self.alt_active = false;
        self.pending_wrap = false;
    }

    /// DECSC / CSI s: save the cursor position.
    fn save_cursor(&mut self) {
        self.saved_cursor = SavedCursor {
            row: self.screen.cursor.row,
            col: self.screen.cursor.col,
        };
    }

    /// DECRC / CSI u: restore the saved cursor position.
    fn restore_cursor(&mut self) {
        let rows = self.screen.rows;
        let cols = self.screen.cols;
        self.screen.cursor.row = self.saved_cursor.row.min(rows - 1);
        self.screen.cursor.col = self.saved_cursor.col.min(cols - 1);
        self.pending_wrap = false;
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
                2 => {} // faint: accepted, not visually distinct here
                3 => self.pen.flags.italic = true,
                4 => self.pen.flags.underline = true,
                7 => self.pen.flags.inverse = true,
                22 => self.pen.flags.bold = false, // normal intensity (clears bold/faint)
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

/// Extract a filesystem path from an OSC 7 `file://host/path` URI, percent-
/// decoding `%XX` escapes. `None` for a non-file URI or empty path.
fn cwd_from_file_uri(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("file://")?;
    // The path begins at the first '/' after the (optional) host.
    let path = &rest[rest.find('/')?..];
    let decoded = percent_decode(path);
    if decoded.is_empty() {
        None
    } else {
        Some(decoded)
    }
}

/// Minimal `%XX` percent-decoding for OSC 7 paths.
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hi = (b[i + 1] as char).to_digit(16);
            let lo = (b[i + 2] as char).to_digit(16);
            if let (Some(h), Some(l)) = (hi, lo) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
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
            b'\n' | 0x0b | 0x0c => {
                self.pending_wrap = false;
                self.line_feed();
            }
            b'\r' => {
                self.pending_wrap = false;
                self.screen.cursor.col = 0;
            }
            b'\t' => {
                self.pending_wrap = false;
                let s = &mut self.screen;
                let next = ((s.cursor.col / 8) + 1) * 8;
                s.cursor.col = next.min(s.cols - 1);
            }
            0x08 => {
                self.pending_wrap = false;
                let s = &mut self.screen;
                if s.cursor.col > 0 {
                    s.cursor.col -= 1;
                }
            }
            0x07 => self.bell = true, // BEL
            _ => {}
        }
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        let code = params.first().and_then(|c| std::str::from_utf8(c).ok());
        match code {
            // Window/icon title.
            Some("0") | Some("1") | Some("2") => {
                if let Some(t) = params.get(1) {
                    self.title = Some(String::from_utf8_lossy(t).into_owned());
                }
            }
            // OSC 7 ; file://host/path — the shell's working directory.
            Some("7") => {
                if let Some(uri) = params.get(1) {
                    let uri = String::from_utf8_lossy(uri);
                    if let Some(path) = cwd_from_file_uri(&uri) {
                        self.cwd = Some(path);
                    }
                }
            }
            // OSC 9 ; <body> — simple desktop notification (iTerm2 convention).
            Some("9") => {
                if let Some(body) = params.get(1) {
                    let body = String::from_utf8_lossy(body).into_owned();
                    if !body.is_empty() {
                        self.notifications.push(("Notification".to_string(), body));
                    }
                }
            }
            // OSC 777 ; notify ; <title> ; <body> (urxvt/extension convention).
            Some("777") => {
                let sub = params.get(1).and_then(|c| std::str::from_utf8(c).ok());
                if sub == Some("notify") {
                    let title = params
                        .get(2)
                        .map(|b| String::from_utf8_lossy(b).into_owned())
                        .unwrap_or_default();
                    let body = params
                        .get(3)
                        .map(|b| String::from_utf8_lossy(b).into_owned())
                        .unwrap_or_default();
                    self.notifications.push((title, body));
                }
            }
            _ => {}
        }
    }

    fn esc_dispatch(&mut self, _intermediates: &[u8], _ignore: bool, byte: u8) {
        match byte {
            b'7' => self.save_cursor(),    // DECSC
            b'8' => self.restore_cursor(), // DECRC
            b'D' => {
                self.pending_wrap = false;
                self.line_feed();
            } // IND
            b'E' => {
                self.pending_wrap = false;
                self.screen.cursor.col = 0;
                self.line_feed();
            } // NEL
            b'M' => {
                self.pending_wrap = false;
                self.reverse_index();
            } // RI
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
        // Cursor moves and edits clear any deferred wrap latch.
        match action {
            'A' => {
                self.pending_wrap = false;
                let n = param_or(params, 1) as usize;
                let s = &mut self.screen;
                s.cursor.row = s.cursor.row.saturating_sub(n);
            }
            'B' => {
                self.pending_wrap = false;
                let n = param_or(params, 1) as usize;
                let s = &mut self.screen;
                s.cursor.row = (s.cursor.row + n).min(s.rows - 1);
            }
            'C' => {
                self.pending_wrap = false;
                let n = param_or(params, 1) as usize;
                let s = &mut self.screen;
                s.cursor.col = (s.cursor.col + n).min(s.cols - 1);
            }
            'D' => {
                self.pending_wrap = false;
                let n = param_or(params, 1) as usize;
                let s = &mut self.screen;
                s.cursor.col = s.cursor.col.saturating_sub(n);
            }
            'H' | 'f' => {
                // CUP / HVP: 1-based row;col -> 0-based.
                self.pending_wrap = false;
                let row = nth_param(params, 0).unwrap_or(1) as usize - 1;
                let col = nth_param(params, 1).unwrap_or(1) as usize - 1;
                let s = &mut self.screen;
                s.cursor.row = row.min(s.rows - 1);
                s.cursor.col = col.min(s.cols - 1);
            }
            'J' => self.erase_display(first_param_raw(params)),
            'K' => self.erase_line(first_param_raw(params)),
            'L' => self.insert_lines(param_or(params, 1) as usize), // IL
            'M' => self.delete_lines(param_or(params, 1) as usize), // DL
            '@' => self.insert_chars(param_or(params, 1) as usize), // ICH
            'P' => self.delete_chars(param_or(params, 1) as usize), // DCH
            'X' => self.erase_chars(param_or(params, 1) as usize),  // ECH
            'r' => {
                // DECSTBM: set top/bottom scroll margins (1-based, inclusive).
                let top = nth_param(params, 0).unwrap_or(0);
                let bottom = nth_param(params, 1).unwrap_or(0);
                self.set_scroll_region(top, bottom);
            }
            's' => self.save_cursor(),    // SCOSC
            'u' => self.restore_cursor(), // SCORC
            'm' => self.apply_sgr(params),
            'h' | 'l' if intermediates.first() == Some(&b'?') => {
                // DEC private mode set/reset.
                let set = action == 'h';
                for p in params.iter() {
                    match p.first().copied() {
                        Some(7) => self.autowrap = set, // DECAWM
                        Some(25) => self.screen.cursor.visible = set, // DECTCEM
                        Some(47) | Some(1047) => {
                            if set {
                                self.enter_alt_screen(false);
                            } else {
                                self.leave_alt_screen(false);
                            }
                        }
                        Some(1049) => {
                            if set {
                                self.enter_alt_screen(true);
                            } else {
                                self.leave_alt_screen(true);
                            }
                        }
                        _ => {}
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
    fn scrollback_accumulates_and_viewport_scrolls_back() {
        let mut t = Terminal::new(2, 6);
        // On a 2-row grid, L0 and L1 scroll into history as L2/L3 arrive.
        t.feed(b"L0\r\nL1\r\nL2\r\nL3");
        assert_eq!(t.scrollback_len(), 2);

        let live = t.viewport(0);
        assert_eq!(live.len(), 2);
        let live_top: String = live[0].iter().map(|c| c.c).collect();
        assert!(live_top.trim_end().starts_with("L2"));

        // Scroll all the way back: the oldest line (L0) is at the top.
        let back = t.viewport(t.scrollback_len());
        let back_top: String = back[0].iter().map(|c| c.c).collect();
        assert!(back_top.trim_end().starts_with("L0"));
    }

    #[test]
    fn search_finds_matches_in_scrollback_and_screen() {
        let mut t = Terminal::new(2, 10);
        // "needle" on the first line scrolls into history; "needle" again live.
        t.feed(b"needle\r\nfiller\r\nneedle");
        let hits = t.search("needle");
        assert_eq!(hits.len(), 2);
        // Case-insensitive, and column reported.
        assert_eq!(t.search("NEEDLE").len(), 2);
        assert_eq!(hits[0].col, 0);
        // First hit is older (lower absolute line) than the second.
        assert!(hits[0].line < hits[1].line);
        assert!(t.search("absent").is_empty());
        assert!(t.search("").is_empty());
    }

    #[test]
    fn alt_screen_does_not_grow_scrollback() {
        let mut t = Terminal::new(2, 6);
        t.feed(b"\x1b[?1049h"); // enter alt screen
        t.feed(b"A\r\nB\r\nC\r\nD");
        assert_eq!(t.scrollback_len(), 0);
    }

    #[test]
    fn wide_char_occupies_two_cells() {
        let mut t = Terminal::new(2, 10);
        t.feed("世界".as_bytes());
        assert_eq!(t.cell(0, 0).c, '世');
        assert_eq!(t.cell(0, 1).c, ' '); // spacer
        assert_eq!(t.cell(0, 2).c, '界');
        assert_eq!(t.cell(0, 3).c, ' '); // spacer
        // Cursor advanced by 2 per wide glyph.
        assert_eq!(t.cursor().col, 4);
        // render_to_string doesn't pad wide glyphs with stray spaces.
        assert!(t.render_to_string().starts_with("世界"));
    }

    #[test]
    fn wide_char_wraps_when_it_cannot_fit() {
        // 3-col grid: 'a' then a wide glyph can't fit in the remaining 2 cols
        // after column 2, so it wraps to the next line.
        let mut t = Terminal::new(2, 3);
        t.feed(b"ab");
        t.feed("世".as_bytes());
        // 'a','b' on row 0; wide glyph wrapped to row 1 col 0.
        assert_eq!(t.cell(1, 0).c, '世');
    }

    #[test]
    fn osc_7_sets_working_directory() {
        let mut t = Terminal::new(3, 10);
        t.feed(b"\x1b]7;file://host/home/me/proj%20x\x07");
        assert_eq!(t.take_cwd().as_deref(), Some("/home/me/proj x"));
        assert!(t.take_cwd().is_none());
        // file:/// (empty host) also works.
        t.feed(b"\x1b]7;file:///srv\x07");
        assert_eq!(t.take_cwd().as_deref(), Some("/srv"));
    }

    #[test]
    fn osc_sets_window_title() {
        let mut t = Terminal::new(3, 10);
        t.feed(b"\x1b]0;my-shell\x07");
        assert_eq!(t.title(), Some("my-shell"));
        assert_eq!(t.take_title().as_deref(), Some("my-shell"));
        assert_eq!(t.title(), None);
    }

    #[test]
    fn osc_9_raises_notification() {
        let mut t = Terminal::new(3, 10);
        t.feed(b"\x1b]9;build finished\x07");
        let n = t.take_notifications();
        assert_eq!(n.len(), 1);
        assert_eq!(n[0], ("Notification".to_string(), "build finished".to_string()));
        assert!(t.take_notifications().is_empty());
    }

    #[test]
    fn osc_777_notify_title_and_body() {
        let mut t = Terminal::new(3, 10);
        t.feed(b"\x1b]777;notify;Claude;needs input\x07");
        let n = t.take_notifications();
        assert_eq!(n, vec![("Claude".to_string(), "needs input".to_string())]);
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

    // ---- New behavior tests ----

    fn row_str(t: &Terminal, n: usize) -> String {
        t.row(n)
            .iter()
            .map(|c| c.c)
            .collect::<String>()
            .trim_end_matches(' ')
            .to_string()
    }

    #[test]
    fn alt_screen_save_and_restore() {
        let mut t = Terminal::new(5, 20);
        t.feed(b"primary");
        assert!(!t.is_alt_screen());
        // Enter alternate screen (1049): saves primary + cursor, clears alt.
        t.feed(b"\x1b[?1049h");
        assert!(t.is_alt_screen());
        assert_eq!(row_str(&t, 0), ""); // alt buffer starts blank
        t.feed(b"\x1b[1;1Halt");
        assert_eq!(row_str(&t, 0), "alt");
        // Leave: primary content restored, cursor restored.
        t.feed(b"\x1b[?1049l");
        assert!(!t.is_alt_screen());
        assert_eq!(row_str(&t, 0), "primary");
        assert_eq!(t.cursor().col, 7);
        assert_eq!(t.cursor().row, 0);
    }

    #[test]
    fn alt_screen_47_no_cursor_save() {
        let mut t = Terminal::new(5, 20);
        t.feed(b"primary");
        t.feed(b"\x1b[?47h");
        assert!(t.is_alt_screen());
        t.feed(b"alt");
        assert_eq!(row_str(&t, 0), "alt");
        t.feed(b"\x1b[?47l");
        assert!(!t.is_alt_screen());
        assert_eq!(row_str(&t, 0), "primary");
    }

    #[test]
    fn decsc_decrc_save_restore_cursor() {
        let mut t = Terminal::new(10, 10);
        t.feed(b"\x1b[3;5H"); // row 2 col 4
        t.feed(b"\x1b7"); // DECSC
        t.feed(b"\x1b[8;8H"); // move elsewhere row 7 col 7
        assert_eq!(t.cursor().row, 7);
        assert_eq!(t.cursor().col, 7);
        t.feed(b"\x1b8"); // DECRC
        assert_eq!(t.cursor().row, 2);
        assert_eq!(t.cursor().col, 4);
    }

    #[test]
    fn csi_s_u_save_restore_cursor() {
        let mut t = Terminal::new(10, 10);
        t.feed(b"\x1b[2;3H"); // row 1 col 2
        t.feed(b"\x1b[s"); // save
        t.feed(b"\x1b[9;9H");
        t.feed(b"\x1b[u"); // restore
        assert_eq!(t.cursor().row, 1);
        assert_eq!(t.cursor().col, 2);
    }

    #[test]
    fn scroll_region_only_scrolls_region() {
        // 6 rows. Margins rows 2..4 (1-based) -> 0-based 1..3.
        let mut t = Terminal::new(6, 10);
        t.feed(b"r0\r\nr1\r\nr2\r\nr3\r\nr4\r\nr5");
        t.feed(b"\x1b[2;4r"); // DECSTBM rows 2..4 (homes cursor to 0,0)
        // Move to bottom margin (row 3, 0-based) and line feed to scroll region.
        t.feed(b"\x1b[4;1H"); // row 3 col 0
        t.feed(b"\nNEW");
        // Rows outside the region are untouched.
        assert_eq!(row_str(&t, 0), "r0");
        assert_eq!(row_str(&t, 4), "r4");
        assert_eq!(row_str(&t, 5), "r5");
        // Region scrolled up: old r2 gone, r3 moved up, NEW at bottom margin.
        assert_eq!(row_str(&t, 1), "r2");
        assert_eq!(row_str(&t, 2), "r3");
        assert_eq!(row_str(&t, 3), "NEW");
    }

    #[test]
    fn reverse_index_scrolls_region_down_at_top() {
        let mut t = Terminal::new(6, 10);
        t.feed(b"r0\r\nr1\r\nr2\r\nr3\r\nr4\r\nr5");
        t.feed(b"\x1b[2;4r"); // margins 0-based 1..3
        t.feed(b"\x1b[2;1H"); // top margin row 1 col 0
        t.feed(b"\x1bM"); // RI at top margin -> scroll region down
        // Top margin now blank, contents pushed down within region.
        assert_eq!(row_str(&t, 0), "r0"); // outside untouched
        assert_eq!(row_str(&t, 1), ""); // new blank at top margin
        assert_eq!(row_str(&t, 2), "r1");
        assert_eq!(row_str(&t, 3), "r2");
        assert_eq!(row_str(&t, 4), "r4"); // outside untouched (r3 fell off region)
    }

    #[test]
    fn ind_and_nel() {
        let mut t = Terminal::new(5, 10);
        t.feed(b"\x1b[2;5Hx"); // row 1, col 4 -> 'x', cursor col 5
        t.feed(b"\x1bD"); // IND: down one, keep column
        assert_eq!(t.cursor().row, 2);
        assert_eq!(t.cursor().col, 5);
        t.feed(b"\x1bE"); // NEL: down one + col 0
        assert_eq!(t.cursor().row, 3);
        assert_eq!(t.cursor().col, 0);
    }

    #[test]
    fn il_dl_within_region() {
        let mut t = Terminal::new(5, 10);
        t.feed(b"a\r\nb\r\nc\r\nd\r\ne");
        // Insert a line at row 1 (1-based row 2).
        t.feed(b"\x1b[2;1H"); // row 1
        t.feed(b"\x1b[L"); // IL 1
        assert_eq!(row_str(&t, 0), "a");
        assert_eq!(row_str(&t, 1), ""); // inserted blank
        assert_eq!(row_str(&t, 2), "b");
        assert_eq!(row_str(&t, 3), "c");
        assert_eq!(row_str(&t, 4), "d"); // 'e' pushed off bottom

        // Now delete the blank line we inserted.
        t.feed(b"\x1b[2;1H");
        t.feed(b"\x1b[M"); // DL 1
        assert_eq!(row_str(&t, 0), "a");
        assert_eq!(row_str(&t, 1), "b");
        assert_eq!(row_str(&t, 2), "c");
        assert_eq!(row_str(&t, 3), "d");
    }

    #[test]
    fn ich_dch_ech() {
        // ICH: insert blanks shifting right.
        let mut t = Terminal::new(2, 10);
        t.feed(b"abcdef");
        t.feed(b"\x1b[1;3H"); // col 2 ('c')
        t.feed(b"\x1b[2@"); // insert 2 blanks
        assert_eq!(row_str(&t, 0), "ab  cdef");

        // DCH: delete chars shifting left.
        let mut t = Terminal::new(2, 10);
        t.feed(b"abcdef");
        t.feed(b"\x1b[1;3H"); // col 2 ('c')
        t.feed(b"\x1b[2P"); // delete 2 chars
        assert_eq!(row_str(&t, 0), "abef");

        // ECH: erase in place.
        let mut t = Terminal::new(2, 10);
        t.feed(b"abcdef");
        t.feed(b"\x1b[1;3H"); // col 2 ('c')
        t.feed(b"\x1b[2X"); // erase 2 chars
        assert_eq!(t.cell(0, 0).c, 'a');
        assert_eq!(t.cell(0, 1).c, 'b');
        assert_eq!(t.cell(0, 2).c, ' ');
        assert_eq!(t.cell(0, 3).c, ' ');
        assert_eq!(t.cell(0, 4).c, 'e');
    }

    #[test]
    fn deferred_wrap_no_premature_blank_line() {
        let mut t = Terminal::new(3, 5);
        t.feed(b"abcde"); // exactly fills row 0; cursor pending at end of row 0
        // The 5 chars are on row 0, nothing wrapped yet.
        assert_eq!(row_str(&t, 0), "abcde");
        assert_eq!(row_str(&t, 1), "");
        assert_eq!(t.cursor().row, 0); // still on row 0 (pending wrap)
        // One more char wraps to row 1 col 0.
        t.feed(b"f");
        assert_eq!(t.cell(1, 0).c, 'f');
        assert_eq!(t.cursor().row, 1);
        assert_eq!(t.cursor().col, 1);
    }

    #[test]
    fn autowrap_disabled_overwrites_in_place() {
        let mut t = Terminal::new(3, 5);
        t.feed(b"\x1b[?7l"); // disable autowrap
        t.feed(b"abcdeXYZ"); // last col keeps overwriting
        assert_eq!(row_str(&t, 0), "abcdZ"); // 'Z' is the last written
        assert_eq!(row_str(&t, 1), ""); // never wrapped
    }

    #[test]
    fn sgr_reset_attributes() {
        let mut t = Terminal::new(2, 20);
        // Set all, then clear each.
        t.feed(b"\x1b[1;3;4;7mA"); // bold italic underline inverse
        let a = *t.cell(0, 0);
        assert!(a.flags.bold && a.flags.italic && a.flags.underline && a.flags.inverse);
        t.feed(b"\x1b[22mB"); // clear bold/faint
        assert!(!t.cell(0, 1).flags.bold);
        assert!(t.cell(0, 1).flags.italic); // others remain
        t.feed(b"\x1b[23mC"); // clear italic
        assert!(!t.cell(0, 2).flags.italic);
        t.feed(b"\x1b[24mD"); // clear underline
        assert!(!t.cell(0, 3).flags.underline);
        t.feed(b"\x1b[27mE"); // clear inverse
        assert!(!t.cell(0, 4).flags.inverse);
        // faint (2) does not error and 0 resets all.
        t.feed(b"\x1b[1;2;4m\x1b[0mF");
        let f = *t.cell(0, 5);
        assert!(!f.flags.bold && !f.flags.underline && !f.flags.italic && !f.flags.inverse);
    }
}
