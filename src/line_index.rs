use lsp_types::{Position, PositionEncodingKind, Range};

/// The position encoding negotiated with the client.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    Utf8,
    Utf16,
}

impl Encoding {
    pub fn kind(self) -> PositionEncodingKind {
        match self {
            Encoding::Utf8 => PositionEncodingKind::UTF8,
            Encoding::Utf16 => PositionEncodingKind::UTF16,
        }
    }
}

/// A single-line span, stored in both encodings so results can be produced
/// without keeping file contents around.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub line: u32,
    pub start8: u32,
    pub end8: u32,
    pub start16: u32,
    pub end16: u32,
}

impl Span {
    pub fn new(text: &str, li: &LineIndex, start: usize, end: usize) -> Span {
        let line = li.line_of(start);
        let ls = li.starts[line];
        let start16 = utf16_len(&text[ls..start]);
        let end16 = start16 + utf16_len(&text[start..end]);
        Span {
            line: line as u32,
            start8: (start - ls) as u32,
            end8: (end - ls) as u32,
            start16,
            end16,
        }
    }

    fn cols(&self, enc: Encoding) -> (u32, u32) {
        match enc {
            Encoding::Utf8 => (self.start8, self.end8),
            Encoding::Utf16 => (self.start16, self.end16),
        }
    }

    pub fn range(&self, enc: Encoding) -> Range {
        let (s, e) = self.cols(enc);
        Range::new(Position::new(self.line, s), Position::new(self.line, e))
    }

    /// Inclusive of the end so a cursor right after a token still hits it.
    pub fn contains(&self, pos: Position, enc: Encoding) -> bool {
        let (s, e) = self.cols(enc);
        pos.line == self.line && s <= pos.character && pos.character <= e
    }
}

fn utf16_len(s: &str) -> u32 {
    s.chars().map(|c| c.len_utf16() as u32).sum()
}

pub struct LineIndex {
    starts: Vec<usize>,
}

impl LineIndex {
    pub fn new(text: &str) -> LineIndex {
        let mut starts = vec![0];
        starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
        LineIndex { starts }
    }

    pub fn line_of(&self, offset: usize) -> usize {
        self.starts.partition_point(|&s| s <= offset) - 1
    }

    pub fn offset(&self, text: &str, pos: Position, enc: Encoding) -> usize {
        let Some(&ls) = self.starts.get(pos.line as usize) else {
            return text.len();
        };
        let le = self
            .starts
            .get(pos.line as usize + 1)
            .copied()
            .unwrap_or(text.len());
        let line = &text[ls..le];
        match enc {
            Encoding::Utf8 => {
                let mut o = (pos.character as usize).min(line.len());
                while !line.is_char_boundary(o) {
                    o -= 1;
                }
                ls + o
            }
            Encoding::Utf16 => {
                let mut units = 0;
                for (i, c) in line.char_indices() {
                    if units >= pos.character {
                        return ls + i;
                    }
                    units += c.len_utf16() as u32;
                }
                le
            }
        }
    }

    pub fn position(&self, text: &str, offset: usize, enc: Encoding) -> Position {
        let line = self.line_of(offset);
        let ls = self.starts[line];
        let col = match enc {
            Encoding::Utf8 => (offset - ls) as u32,
            Encoding::Utf16 => utf16_len(&text[ls..offset]),
        };
        Position::new(line as u32, col)
    }
}
