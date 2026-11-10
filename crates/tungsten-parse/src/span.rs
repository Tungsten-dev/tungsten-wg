//! Spans and diagnostics. A diagnostic carries the sources it points into, with their display
//! names, and renders with `codespan-reporting`.

use std::collections::HashMap;
use std::fmt;
use std::ops::Range;
use std::sync::Arc;

use codespan_reporting::diagnostic::{Diagnostic as Report, Label};
use codespan_reporting::files::{self, Files, SimpleFile};
use codespan_reporting::term::{self, Chars, Config};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct FileId(pub u32);

/// A byte range in one source file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Span {
    pub file: FileId,
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub fn new(file: FileId, start: usize, end: usize) -> Self {
        Self {
            file,
            start: start as u32,
            end: end as u32,
        }
    }

    pub fn to(self, other: Span) -> Span {
        Span {
            file: self.file,
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Lex,
    Parse,
    Elab,
}

/// Source files by id, each with a display name (a path, or `file<N>` when nobody named it).
#[derive(Clone, Default)]
pub struct SourceFiles {
    files: HashMap<FileId, SimpleFile<String, Arc<str>>>,
}

impl SourceFiles {
    pub fn single(file: FileId, name: Option<String>, src: &str) -> SourceFiles {
        let mut s = SourceFiles::default();
        s.add(file, name, src);
        s
    }

    /// Replaces any earlier file with the same id.
    pub fn add(&mut self, file: FileId, name: Option<String>, src: &str) {
        let name = name.unwrap_or_else(|| format!("file{}", file.0));
        self.files.insert(file, SimpleFile::new(name, src.into()));
    }

    pub fn source(&self, file: FileId) -> Option<&str> {
        self.files.get(&file).map(|f| f.source().as_ref())
    }

    pub fn name(&self, file: FileId) -> Option<&str> {
        self.files.get(&file).map(|f| f.name().as_str())
    }

    fn get(&self, file: FileId) -> Result<&SimpleFile<String, Arc<str>>, files::Error> {
        self.files.get(&file).ok_or(files::Error::FileMissing)
    }

    /// The span's byte range, clamped to the source and to char boundaries.
    fn range(&self, span: Span) -> Option<Range<usize>> {
        let src = self.source(span.file)?;
        let start = floor_boundary(src, (span.start as usize).min(src.len()));
        let end = floor_boundary(src, (span.end as usize).clamp(start, src.len()));
        Some(start..end)
    }
}

impl fmt::Debug for SourceFiles {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut names: Vec<_> = self.files.iter().map(|(id, f)| (id.0, f.name())).collect();
        names.sort();
        f.debug_map().entries(names).finish()
    }
}

impl<'a> Files<'a> for SourceFiles {
    type FileId = FileId;
    type Name = &'a str;
    type Source = &'a str;

    fn name(&'a self, id: FileId) -> Result<&'a str, files::Error> {
        Ok(self.get(id)?.name())
    }
    fn source(&'a self, id: FileId) -> Result<&'a str, files::Error> {
        Ok(self.get(id)?.source())
    }
    fn line_index(&'a self, id: FileId, byte_index: usize) -> Result<usize, files::Error> {
        self.get(id)?.line_index((), byte_index)
    }
    fn line_range(&'a self, id: FileId, line_index: usize) -> Result<Range<usize>, files::Error> {
        self.get(id)?.line_range((), line_index)
    }
}

/// Where a diagnostic points: the line, for tests and tools that want more than the message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snippet {
    pub line: usize,
    /// 1-based, in Unicode scalar values.
    pub col: usize,
    /// The source line, without its line ending.
    pub text: String,
    /// Width of the underline in scalar values (at least 1).
    pub width: usize,
}

impl Snippet {
    pub fn new(src: &str, span: Span) -> Snippet {
        let start = floor_boundary(src, (span.start as usize).min(src.len()));
        let end = floor_boundary(src, (span.end as usize).clamp(start, src.len()));
        let line_start = src[..start].rfind('\n').map_or(0, |i| i + 1);
        let line_end = src[start..].find('\n').map_or(src.len(), |i| start + i);
        Snippet {
            line: src[..line_start].matches('\n').count() + 1,
            col: src[line_start..start].chars().count() + 1,
            text: src[line_start..line_end].trim_end_matches('\r').to_string(),
            width: src[start..end.min(line_end).max(start)]
                .chars()
                .count()
                .max(1),
        }
    }
}

fn floor_boundary(s: &str, mut i: usize) -> usize {
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub stage: Stage,
    pub span: Span,
    pub message: String,
    /// Shown as secondary labels when their file is known, as plain notes otherwise.
    pub notes: Vec<(Span, String)>,
    files: Option<Arc<SourceFiles>>,
}

impl Diagnostic {
    pub fn new(stage: Stage, span: Span, message: impl Into<String>) -> Self {
        Self {
            stage,
            span,
            message: message.into(),
            notes: vec![],
            files: None,
        }
    }

    pub fn lex(span: Span, message: impl Into<String>) -> Self {
        Self::new(Stage::Lex, span, message)
    }

    pub fn parse(span: Span, message: impl Into<String>) -> Self {
        Self::new(Stage::Parse, span, message)
    }

    pub fn elab(span: Span, message: impl Into<String>) -> Self {
        Self::new(Stage::Elab, span, message)
    }

    pub fn note(mut self, span: Span, message: impl Into<String>) -> Self {
        self.notes.push((span, message.into()));
        self
    }

    /// Attaches the sources to render with, replacing any attached earlier.
    pub fn with_files(mut self, files: impl Into<Arc<SourceFiles>>) -> Self {
        self.files = Some(files.into());
        self
    }

    pub fn files(&self) -> Option<&SourceFiles> {
        self.files.as_deref()
    }

    /// The display name of the file the diagnostic points into.
    pub fn file_name(&self) -> Option<&str> {
        self.files()?.name(self.span.file)
    }

    pub fn snippet(&self) -> Option<Snippet> {
        Some(Snippet::new(
            self.files()?.source(self.span.file)?,
            self.span,
        ))
    }

    fn report(&self, files: &SourceFiles) -> Option<Report<FileId>> {
        let mut labels = vec![Label::primary(self.span.file, files.range(self.span)?)];
        let mut notes = vec![];
        for (span, note) in &self.notes {
            match files.range(*span) {
                Some(r) => labels.push(Label::secondary(span.file, r).with_message(note)),
                None => notes.push(note.clone()),
            }
        }
        Some(
            Report::error()
                .with_message(&self.message)
                .with_labels(labels)
                .with_notes(notes),
        )
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(files) = self.files()
            && let Some(report) = self.report(files)
        {
            let config = Config {
                chars: Chars::ascii(),
                ..Config::default()
            };
            if let Ok(s) = term::emit_into_string(&config, files, &report) {
                return f.write_str(s.trim_end());
            }
        }
        write!(f, "error: {}", self.message)?;
        for (_, note) in &self.notes {
            write!(f, "\n  note: {note}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Diagnostic {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_name_line_col_and_caret() {
        let src = "fn a() {\n    é = 1;\n}\n";
        let at = src.find('=').unwrap();
        let d = Diagnostic::elab(Span::new(FileId(0), at, at + 1), "bad")
            .with_files(SourceFiles::single(FileId(0), Some("a.tw".into()), src));
        let s = d.to_string();
        assert!(s.starts_with("error: bad"), "{s}");
        assert!(s.contains("a.tw:2:7"), "{s}");
        assert!(s.contains("    é = 1;"), "{s}");
        assert!(s.contains("      ^"), "{s}");
        assert_eq!(d.snippet().unwrap().col, 7);
    }

    #[test]
    fn notes_have_locations() {
        let src = "one\ntwo\n";
        let files = SourceFiles::single(FileId(3), None, src);
        let d = Diagnostic::elab(Span::new(FileId(3), 4, 7), "second")
            .note(Span::new(FileId(3), 0, 3), "first here")
            .note(Span::new(FileId(9), 0, 1), "elsewhere")
            .with_files(files);
        let s = d.to_string();
        assert!(s.contains("file3:2:1"), "{s}");
        assert!(s.contains("--- first here"), "{s}");
        assert!(s.contains("= elsewhere"), "{s}");
    }

    #[test]
    fn without_files_only_the_message() {
        let d = Diagnostic::parse(Span::default(), "eof").note(Span::default(), "n");
        assert_eq!(d.to_string(), "error: eof\n  note: n");
    }

    #[test]
    fn span_past_end_is_clamped() {
        let d = Diagnostic::parse(Span::new(FileId(0), 100, 200), "eof")
            .with_files(SourceFiles::single(FileId(0), None, "ab"));
        assert_eq!(d.snippet().unwrap().col, 3);
        assert!(d.to_string().contains("file0:1:3"), "{d}");
    }
}
