//! Source positions.
//!
//! Lines and columns are 1-based. Columns count Unicode scalar values from the
//! start of the line (not bytes and not UTF-16 code units).

use std::fmt;

use serde::{Deserialize, Serialize};

/// Index of a source file in the file table that accompanies an IR (see
/// [`crate::ir::FileIr::sources`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct FileId(pub u32);

/// A line/column range inside a single source file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Location {
    pub line: u32,
    pub column: u32,
    pub end_line: u32,
    pub end_column: u32,
    /// The file the range belongs to when it is not the file of the function
    /// that contains it: operations inlined from a function declared in
    /// another module keep their own file. `None` means "the function's file".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<FileId>,
}

impl Location {
    /// Attach a file path, producing a location suitable for diagnostics.
    /// The location's own file, if any, is ignored; see [`Self::resolve`].
    pub fn in_file(self, file: &str) -> SourceLocation {
        SourceLocation {
            file: file.to_owned(),
            line: self.line,
            column: self.column,
            end_line: self.end_line,
            end_column: self.end_column,
        }
    }

    /// The location in its own file (`sources[id]`), or in `default` when it
    /// has none.
    pub fn resolve(self, default: &str, sources: &[String]) -> SourceLocation {
        let file = self
            .file
            .and_then(|id| sources.get(id.0 as usize))
            .map_or(default, String::as_str);
        self.in_file(file)
    }

    /// `line:column`, prefixed with the file path when the location belongs
    /// to another file than `default`.
    pub fn describe(self, default: &str, sources: &[String]) -> String {
        match self.file.and_then(|id| sources.get(id.0 as usize)) {
            Some(file) if file != default => format!("{file}:{}:{}", self.line, self.column),
            _ => format!("{}:{}", self.line, self.column),
        }
    }
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.line, self.column)
    }
}

/// A [`Location`] together with the file it belongs to.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceLocation {
    pub file: String,
    pub line: u32,
    pub column: u32,
    pub end_line: u32,
    pub end_column: u32,
}

impl fmt::Display for SourceLocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}:{}", self.file, self.line, self.column)
    }
}

/// How the locations of one function's IR map to files: `file` is the
/// function's own file, `others` the file table its [`Location::file`] ids
/// index.
#[derive(Clone, Copy, Debug)]
pub struct Sources<'a> {
    pub file: &'a str,
    pub others: &'a [String],
}

impl<'a> Sources<'a> {
    /// A single file, without locations in other files.
    pub fn single(file: &'a str) -> Self {
        Self { file, others: &[] }
    }

    pub fn resolve(&self, location: Location) -> SourceLocation {
        location.resolve(self.file, self.others)
    }

    /// `line:column`, or `file:line:column` for a location in another file.
    pub fn at(&self, location: Location) -> String {
        location.describe(self.file, self.others)
    }

    /// Whether the location belongs to another file than the function's.
    pub fn is_other_file(&self, location: Location) -> bool {
        location
            .file
            .and_then(|id| self.others.get(id.0 as usize))
            .is_some_and(|file| file != self.file)
    }
}

/// Maps byte offsets of a source text to [`Location`]s.
#[derive(Debug)]
pub struct LineIndex<'s> {
    source: &'s str,
    line_starts: Vec<u32>,
}

impl<'s> LineIndex<'s> {
    pub fn new(source: &'s str) -> Self {
        let mut line_starts = vec![0];
        line_starts.extend(
            source
                .bytes()
                .enumerate()
                .filter(|&(_, byte)| byte == b'\n')
                .map(|(index, _)| offset_u32(index + 1)),
        );
        Self {
            source,
            line_starts,
        }
    }

    /// 1-based `(line, column)` of a byte offset. Offsets past the end are clamped.
    pub fn position(&self, offset: u32) -> (u32, u32) {
        let offset = self.clamp(offset);
        let line_index = match self.line_starts.binary_search(&offset) {
            Ok(index) => index,
            Err(index) => index - 1,
        };
        let line_start = self.line_starts[line_index] as usize;
        let column = self.source[line_start..offset as usize].chars().count();
        (offset_u32(line_index + 1), offset_u32(column + 1))
    }

    /// Location covering the byte range `start..end`.
    pub fn location(&self, start: u32, end: u32) -> Location {
        let (line, column) = self.position(start);
        let (end_line, end_column) = self.position(end.max(start));
        Location {
            line,
            column,
            end_line,
            end_column,
            file: None,
        }
    }

    /// Text of a 1-based line without its line terminator.
    pub fn line_text(&self, line: u32) -> Option<&'s str> {
        let index = usize::try_from(line).ok()?.checked_sub(1)?;
        let start = *self.line_starts.get(index)? as usize;
        let end = self
            .line_starts
            .get(index + 1)
            .map_or(self.source.len(), |&next| next as usize);
        Some(self.source[start..end].trim_end_matches(['\n', '\r']))
    }

    fn clamp(&self, offset: u32) -> u32 {
        let mut offset = (offset as usize).min(self.source.len());
        while !self.source.is_char_boundary(offset) {
            offset -= 1;
        }
        offset_u32(offset)
    }
}

fn offset_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_are_one_based() {
        let index = LineIndex::new("ab\ncd\n");
        assert_eq!(index.position(0), (1, 1));
        assert_eq!(index.position(1), (1, 2));
        assert_eq!(index.position(3), (2, 1));
        assert_eq!(index.position(5), (2, 3));
        assert_eq!(index.position(6), (3, 1));
    }

    #[test]
    fn columns_count_characters_not_bytes() {
        let index = LineIndex::new("é = 1;\n");
        // `=` is the third character but the fourth byte.
        assert_eq!(index.position(3), (1, 3));
    }

    #[test]
    fn line_text_strips_terminators() {
        let index = LineIndex::new("first\r\nsecond");
        assert_eq!(index.line_text(1), Some("first"));
        assert_eq!(index.line_text(2), Some("second"));
        assert_eq!(index.line_text(3), None);
        assert_eq!(index.line_text(0), None);
    }

    #[test]
    fn out_of_range_offsets_are_clamped() {
        let index = LineIndex::new("x");
        assert_eq!(index.position(100), (1, 2));
    }
}
