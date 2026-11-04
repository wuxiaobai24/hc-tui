//! Copy Mode：在 Pane 里用键盘移动一个光标，选区直接取本地 VT 的文本。
//!
//! 只维护「视口坐标 + 视口文本快照」，复制时再向 [`TerminalView`] 要真实文本，
//! 这样软换行的行在复制时会被拼回一段，而不是按屏幕宽度断行。

use crate::term::view::TerminalView;

#[derive(Debug)]
pub struct CopyMode {
    pub pane: u32,
    /// 视口坐标，左上角为 (0, 0)。
    pub cursor: (u16, u16),
    /// 选区起点；`None` 表示没有选区。
    pub anchor: Option<(u16, u16)>,
    /// 视口文本快照，用于行内移动与搜索。
    pub lines: Vec<String>,
    pub size: (u16, u16),
    pub search: Option<String>,
    /// 正在输入搜索词。
    pub typing: bool,
    /// 矩形选区（按列取方块）。
    pub rectangle: bool,
    /// 当前搜索命中的格子，用于高亮。
    pub matches: Vec<(u16, u16)>,
}

impl CopyMode {
    pub fn new(
        pane: u32,
        size: (u16, u16),
        lines: Vec<String>,
        cursor: Option<(u16, u16)>,
    ) -> Self {
        let cursor = cursor.unwrap_or((0, size.1.saturating_sub(1)));
        Self {
            pane,
            cursor: (
                cursor.0.min(size.0.saturating_sub(1)),
                cursor.1.min(size.1.saturating_sub(1)),
            ),
            anchor: None,
            lines,
            size,
            search: None,
            typing: false,
            rectangle: false,
            matches: Vec::new(),
        }
    }

    pub fn refresh(&mut self, size: (u16, u16), lines: Vec<String>) {
        self.size = size;
        self.lines = lines;
        self.cursor = (
            self.cursor.0.min(size.0.saturating_sub(1)),
            self.cursor.1.min(size.1.saturating_sub(1)),
        );
        self.recompute_matches();
    }

    pub fn toggle_rectangle(&mut self) {
        self.rectangle = !self.rectangle;
    }

    /// 重新扫描可见行里的搜索命中。
    pub fn recompute_matches(&mut self) {
        self.matches.clear();
        let Some(needle) = self.search.clone() else {
            return;
        };
        if needle.is_empty() {
            return;
        }
        for (row, line) in self.lines.iter().enumerate() {
            let mut offset = 0;
            while let Some(found) = line[offset..].find(&needle) {
                let column = line[..offset + found].chars().count() as u16;
                if column < self.size.0 && row < self.size.1 as usize {
                    self.matches.push((column, row as u16));
                }
                offset += found + needle.len().max(1);
                if offset >= line.len() {
                    break;
                }
            }
        }
    }

    fn line(&self, row: u16) -> &str {
        self.lines
            .get(row as usize)
            .map(String::as_str)
            .unwrap_or("")
    }

    pub fn move_by(&mut self, dx: i32, dy: i32) {
        let x = (self.cursor.0 as i32 + dx).clamp(0, self.size.0.saturating_sub(1) as i32);
        let y = (self.cursor.1 as i32 + dy).clamp(0, self.size.1.saturating_sub(1) as i32);
        self.cursor = (x as u16, y as u16);
    }

    pub fn line_start(&mut self) {
        self.cursor.0 = 0;
    }

    pub fn line_end(&mut self) {
        let length = self.line(self.cursor.1).chars().count() as u16;
        self.cursor.0 = length.saturating_sub(1).min(self.size.0.saturating_sub(1));
    }

    pub fn to_top(&mut self) {
        self.cursor.1 = 0;
    }

    pub fn to_bottom(&mut self) {
        self.cursor.1 = self.size.1.saturating_sub(1);
    }

    /// 下一个词首。跨行继续找。
    pub fn word_next(&mut self) {
        let mut row = self.cursor.1;
        let mut column = self.cursor.0;
        loop {
            let chars: Vec<char> = self.line(row).chars().collect();
            if column as usize >= chars.len() {
                if row + 1 >= self.size.1 {
                    return;
                }
                row += 1;
                column = 0;
                if !self.line(row).trim().is_empty() {
                    break;
                }
                continue;
            }
            let at = column as usize;
            let word = |c: char| c.is_alphanumeric() || c == '_';
            if word(chars[at]) {
                let mut at = at;
                while at < chars.len() && word(chars[at]) {
                    at += 1;
                }
                while at < chars.len() && !word(chars[at]) {
                    at += 1;
                }
                if at < chars.len() {
                    column = at as u16;
                    break;
                }
                if row + 1 >= self.size.1 {
                    return;
                }
                row += 1;
                column = 0;
                if !self.line(row).trim().is_empty() {
                    break;
                }
                continue;
            }
            column = column.saturating_add(1).min(chars.len() as u16);
        }
        self.cursor = (column.min(self.size.0.saturating_sub(1)), row);
    }

    /// 上一个词首。
    pub fn word_previous(&mut self) {
        let mut row = self.cursor.1;
        let mut column = self.cursor.0;
        loop {
            if column == 0 {
                if row == 0 {
                    return;
                }
                row -= 1;
                column = self.line(row).chars().count() as u16;
                if self.line(row).trim().is_empty() {
                    continue;
                }
                continue;
            }
            column -= 1;
            let chars: Vec<char> = self.line(row).chars().collect();
            let word = |c: char| c.is_alphanumeric() || c == '_';
            if let Some(character) = chars.get(column as usize) {
                if word(*character) {
                    while column > 0 && chars.get(column as usize - 1).is_some_and(|c| word(*c)) {
                        column -= 1;
                    }
                    break;
                }
            }
        }
        self.cursor = (column.min(self.size.0.saturating_sub(1)), row);
    }

    pub fn toggle_anchor(&mut self) {
        self.anchor = match self.anchor {
            Some(_) => None,
            None => Some(self.cursor),
        };
    }

    /// 选区的两个端点，按先序排列。
    pub fn selection(&self) -> Option<((u16, u16), (u16, u16))> {
        let anchor = self.anchor?;
        let a = (anchor.1, anchor.0);
        let b = (self.cursor.1, self.cursor.0);
        Some(if a <= b {
            ((anchor.0, anchor.1), self.cursor)
        } else {
            (self.cursor, (anchor.0, anchor.1))
        })
    }

    /// 从光标向后找搜索词，命中就移动光标；`search` 为空返回 `false`。
    pub fn find_next(&mut self) -> bool {
        let Some(needle) = self.search.clone() else {
            return false;
        };
        if needle.is_empty() {
            return false;
        }
        let start_row = self.cursor.1 as usize;
        let start_col = self.cursor.0 as usize;
        for offset in 0..self.lines.len() {
            let row = (start_row + offset) % self.lines.len().max(1);
            let line = &self.lines[row];
            let from = if offset == 0 {
                start_col.min(line.len())
            } else {
                0
            };
            if let Some(found) = line[from..].find(&needle) {
                let column = line[..from + found].chars().count() as u16;
                self.cursor = (column.min(self.size.0.saturating_sub(1)), row as u16);
                self.anchor = Some(self.cursor);
                return true;
            }
        }
        false
    }

    /// 取选中的文本；没有选区时取整行。
    pub fn text(&self, view: &TerminalView) -> String {
        let (start, end) = match self.selection() {
            Some(selection) => selection,
            None => (
                (0, self.cursor.1),
                (self.size.0.saturating_sub(1), self.cursor.1),
            ),
        };
        view.selection_text(start, end, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode() -> CopyMode {
        CopyMode::new(
            1,
            (20, 3),
            vec![
                "hello world".to_string(),
                "second line".to_string(),
                String::new(),
            ],
            Some((0, 0)),
        )
    }

    #[test]
    fn movement_stays_inside_the_viewport() {
        let mut copy = mode();
        copy.move_by(-5, -5);
        assert_eq!(copy.cursor, (0, 0));
        copy.move_by(100, 100);
        assert_eq!(copy.cursor, (19, 2));
    }

    #[test]
    fn selection_is_normalized() {
        let mut copy = mode();
        copy.cursor = (4, 1);
        copy.anchor = Some((3, 0));
        assert_eq!(copy.selection(), Some(((3, 0), (4, 1))));
        copy.cursor = (1, 0);
        assert_eq!(copy.selection(), Some(((1, 0), (3, 0))));
        copy.anchor = Some((4, 1));
        assert_eq!(copy.selection(), Some(((1, 0), (4, 1))));
    }

    #[test]
    fn word_motion_crosses_lines() {
        let mut copy = mode();
        copy.word_next();
        assert_eq!(copy.cursor, (6, 0));
        copy.word_next();
        assert_eq!(copy.cursor, (0, 1));
        copy.word_previous();
        assert_eq!(copy.cursor, (6, 0));
    }

    #[test]
    fn line_end_and_search() {
        let mut copy = mode();
        copy.line_end();
        assert_eq!(copy.cursor.0, 10);
        copy.cursor = (0, 0);
        copy.search = Some("line".to_string());
        assert!(copy.find_next());
        assert_eq!(copy.cursor, (7, 1));
    }
}
