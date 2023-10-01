// RCL -- A sane configuration language.
// Copyright 2023 Ruud van Asseldonk

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// A copy of the License has been included in the root of the repository.

//! Utilities for pretty-printing code.
//!
//! The approach in this module is inspired by Philip Wadler’s 2003 paper
//! [_A Prettier Printer_][wadler2003]. The implementation is different to make
//! it more suitable for Rust’s strict evaluation, and new constructors are
//! added to deal with with some formatting edge cases. For example, newlines
//! in string literals should be preserved without creating indentation after
//! the newline.
//!
//! [wadler2003]: https://homepages.inf.ed.ac.uk/wadler/papers/prettier/prettier.pdf

use crate::markup::{Markup, MarkupMode};
use crate::platform_utils::CouldBeTerminal;
use crate::pprint::printer::{PrintResult, Printer};

/// Whether to format a node in wide mode or tall mode.
#[derive(Copy, Clone)]
enum Mode {
    Wide,
    Tall,
}

/// Configuration for the pretty-printer.
pub struct Config {
    /// The pretty printer will try to avoid creating lines longer than `width`
    /// columns, but this is not always possible.
    pub width: u32,

    /// How to output color and other markup hints.
    pub markup: MarkupMode,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            // Although Black found a limit of 88 to produce significantly
            // shorter files for Python than sticking strictly to 80 (see also
            // https://black.readthedocs.io/en/stable/the_black_code_style/current_style.html#line-length
            // ), in my tests (based on not that much data so far) I preferred
            // just 80.
            width: 80,
            markup: MarkupMode::None,
        }
    }
}

impl Config {
    /// Get the default print configuration for a file descriptor.
    pub fn default_for_fd<T: CouldBeTerminal>(fd: &T) -> Config {
        let markup = if fd.should_color() {
            MarkupMode::Ansi
        } else {
            MarkupMode::None
        };
        Config {
            markup,
            ..Config::default()
        }
    }
}

/// A document tree that can be pretty-printed.
///
/// Every node can be printed in two ways: wide or tall. The goal of the
/// pretty-printer is to print as many nodes as possible in wide mode without
/// exceeding the line width limit. When a node is printed in tall mode, all of
/// its parents must be printed in tall mode as well.
///
/// The main mechanism to control layout is the _group_, represented by
/// [`Doc::Group`] and created with `group!`. Every group causes the formatter
/// to make a wide/tall choice, but the contents of the group is either all wide
/// or all tall.
///
/// Indentation is only output in tall mode; in wide mode this behaves like
/// a non-indented group. Note that a `Doc::Indent` and `Doc::Concat` do not
/// themselves represent the ability to choose wide or tall mode, only
/// `Doc::Group` does. Consider this example:
///
/// ```text
/// group! {
///   "["
///   SoftBreak
///   indent! {
///     "elem0" "," Sep
///     "elem1" "," Sep
///     "elem2" Doc::tall(',')
///   }
///   SoftBreak
///   "]"
/// }
/// ```
///
/// If we would allow making a wide/tall decision for the `Doc::Indent`
/// node, then the following three outputs are valid:
///
/// ```text
/// // Wide-wide
/// [elem0, elem1, elem2]
///
/// // Tall-wide
/// [
///   elem0, elem1, elem2
/// ]
///
/// // Tall-tall
/// [
///   elem0,
///   elem1,
///   elem2,
/// ]
/// ```
///
/// If we only make a wide/tall decision at `Doc::Group`, then only the
/// first and last outputs are valid, so this gives us more control: the
/// middle example can still be produced, by wrapping the `Doc::Indent` in
/// a `Doc::Group`.
#[derive(Clone, Debug)]
pub enum Doc<'a> {
    /// A string slice to be spliced into the output.
    Str { content: &'a str, width: u32 },

    /// An owned string to be spliced into the output.
    String { content: String, width: u32 },

    /// Text which is only output in tall mode.
    ///
    /// This can be used to add trailing commas in a collection when the
    /// collection is formatted in tall mode, without having those trailing
    /// commas in the wide mode.
    WhenTall { content: &'static str, width: u32 },

    /// A space in wide mode; a newline in tall mode.
    Sep,

    /// An empty string in wide mode; a newline in tall mode.
    SoftBreak,

    /// A newline. Forces tall mode onto all its parents.
    HardBreak,

    /// A newline without indentation after it. Forces tall mode onto its parents.
    ///
    /// This can be used to preserve string literals in which whitespace is
    /// significant.
    RawBreak,

    /// A concatenation of document fragments.
    Concat(Vec<Doc<'a>>),

    /// A group can be formatted either in wide mode or in tall mode.
    Group(Box<Doc<'a>>),

    /// An indented block.
    Indent(Box<Doc<'a>>),

    /// A newline plus indented block.
    ///
    /// If we are still at the start of a line, then do not emit a newline and
    /// do not increase the indent. If there is already content on the line,
    /// then emit a newline and increase the indent. This can be used for
    /// content that should "hang under" something in its entirety, rather than
    /// having its opening fragments still on the same line.
    ///
    /// In wide mode, this is a no-op.
    FlushIndent(Box<Doc<'a>>),

    /// Apply markup to the inner document.
    Markup(Markup, Box<Doc<'a>>),
}

impl<'a> Doc<'a> {
    /// Construct an empty document.
    pub fn empty() -> Doc<'a> {
        // We pick concat of empty vec over str of empty string, so that when
        // this later gets concatenated with something else, it disappears.
        // An empty string would stick around as a node unless we added logic to
        // omit it during concat.
        Doc::Concat(Vec::new())
    }

    /// Construct a new document fragment from a string slice.
    pub fn str(value: &'a str) -> Doc<'a> {
        use unicode_width::UnicodeWidthStr;
        debug_assert!(
            !value.contains('\n'),
            "Doc fragments cannot contain newlines, use SoftBreak etc.",
        );
        Doc::Str {
            width: value.width() as u32,
            content: value,
        }
    }

    /// Construct a new document fragment from a string slice that may contain newlines.
    ///
    /// The line breaks are converted into hard breaks in the document.
    pub fn lines(value: &'a str) -> Doc<'a> {
        let mut result = Vec::new();
        let mut remainder = value;

        while let Some(i) = remainder.find('\n') {
            if i > 0 {
                result.push(Doc::str(&remainder[..i]));
            }
            result.push(Doc::HardBreak);
            remainder = &remainder[i + 1..];
        }

        if !remainder.is_empty() {
            result.push(Doc::str(remainder));
        }

        match result.len() {
            1 => result.pop().expect("We have one element to pop."),
            _ => Doc::Concat(result),
        }
    }

    /// Construct a new document fragment from an owned string.
    pub fn string(value: String) -> Doc<'a> {
        use unicode_width::UnicodeWidthStr;
        debug_assert!(
            !value.contains('\n'),
            "Doc fragments cannot contain newlines, use SoftBreak etc.",
        );
        Doc::String {
            width: value.width() as u32,
            content: value,
        }
    }

    /// Construct a new document fragment that only gets emitted in tall mode.
    pub fn tall(value: &'static str) -> Doc<'static> {
        use unicode_width::UnicodeWidthStr;
        debug_assert!(
            !value.contains('\n'),
            "Doc fragments cannot contain newlines, use SoftBreak etc.",
        );
        Doc::WhenTall {
            width: value.width() as u32,
            content: value,
        }
    }

    /// Join multiple documents with a separator in between.
    pub fn join<I: Iterator<Item = Doc<'a>>>(elements: I, separator: Doc<'a>) -> Doc<'a> {
        let mut result = Vec::new();
        let mut is_first = true;
        for elem in elements {
            if !is_first {
                result.push(separator.clone());
            } else {
                is_first = false;
            }
            result.push(elem)
        }
        Doc::Concat(result)
    }

    pub fn with_markup(self, markup: Markup) -> Doc<'a> {
        Doc::Markup(markup, Box::new(self))
    }

    /// Clone all strings and make them owned.
    pub fn into_owned(self) -> Doc<'static> {
        match self {
            Doc::Str { content, width } => Doc::String {
                content: content.to_string(),
                width,
            },
            Doc::String { content, width } => Doc::String { content, width },
            Doc::WhenTall { content, width } => Doc::WhenTall { content, width },
            Doc::Sep => Doc::Sep,
            Doc::SoftBreak => Doc::SoftBreak,
            Doc::HardBreak => Doc::HardBreak,
            Doc::RawBreak => Doc::RawBreak,
            Doc::Concat(children) => {
                Doc::Concat(children.into_iter().map(|c| c.into_owned()).collect())
            }
            Doc::Group(inner) => Doc::Group(Box::new(inner.into_owned())),
            Doc::Indent(inner) => Doc::Indent(Box::new(inner.into_owned())),
            Doc::FlushIndent(inner) => Doc::FlushIndent(Box::new(inner.into_owned())),
            Doc::Markup(m, inner) => Doc::Markup(m, Box::new(inner.into_owned())),
        }
    }

    /// Whether any of the nodes in this tree force tall mode.
    ///
    /// A hard break forces tall mode.
    fn is_forced_tall(&self) -> bool {
        match self {
            Doc::HardBreak => true,
            Doc::RawBreak => true,
            Doc::Concat(children) => children.iter().any(|node| node.is_forced_tall()),
            Doc::Group(inner) => inner.is_forced_tall(),
            Doc::Indent(inner) => inner.is_forced_tall(),
            Doc::FlushIndent(inner) => inner.is_forced_tall(),
            Doc::Markup(_, inner) => inner.is_forced_tall(),
            _ => false,
        }
    }

    /// Print the document to the given printer.
    fn print_to(&self, printer: &mut Printer, mode: Mode) -> PrintResult {
        match self {
            Doc::Str { content, width } => printer.push_str(content, *width),
            Doc::String { content, width } => printer.push_str(content, *width),
            Doc::WhenTall { content, width } => match mode {
                Mode::Tall => printer.push_str(content, *width),
                Mode::Wide => PrintResult::Fits,
            },
            Doc::Sep => match mode {
                Mode::Tall => printer.newline(),
                Mode::Wide => printer.push_char(' '),
            },
            Doc::SoftBreak => match mode {
                Mode::Tall => printer.newline(),
                Mode::Wide => PrintResult::Fits,
            },
            Doc::HardBreak => match mode {
                Mode::Tall => printer.newline(),
                Mode::Wide => unreachable!("HardBreak forces Tall mode."),
            },
            Doc::RawBreak => match mode {
                Mode::Tall => printer.raw_newline(),
                Mode::Wide => unreachable!("RawBreak forces Tall mode."),
            },
            Doc::Concat(children) => children.iter().fold(PrintResult::Fits, |r, doc| {
                doc.print_to(printer, mode).max(r)
            }),
            Doc::Group(inner) => {
                if inner.is_forced_tall() {
                    debug_assert!(matches!(mode, Mode::Tall));
                    return inner.print_to(printer, mode);
                }

                match mode {
                    // If we are wide, then the inner content must be wide too.
                    Mode::Wide => inner.print_to(printer, mode),

                    // If we are tall, then we can try to make the inner content
                    // wide. If that is too wide, then we backtrack and try to
                    // make it tall instead.
                    Mode::Tall => match printer.try_(|p| inner.print_to(p, Mode::Wide)) {
                        PrintResult::Overflow => inner.print_to(printer, Mode::Tall),
                        PrintResult::Fits => PrintResult::Fits,
                    },
                }
            }
            Doc::Indent(inner) => match mode {
                Mode::Wide => inner.print_to(printer, mode),
                Mode::Tall => printer.indented(|p| inner.print_to(p, mode)),
            },
            Doc::FlushIndent(inner) => match mode {
                Mode::Wide => inner.print_to(printer, mode),
                Mode::Tall => {
                    if printer.flush_newline() {
                        printer.indented(|p| inner.print_to(p, mode))
                    } else {
                        inner.print_to(printer, mode)
                    }
                }
            },
            Doc::Markup(markup, inner) => printer.with_markup(*markup, |p| inner.print_to(p, mode)),
        }
    }

    /// Pretty-print the document. Ensure the document ends in a newline.
    pub fn println(&self, config: &Config) -> String {
        let mut printer = Printer::new(config);
        self.print_to(&mut printer, Mode::Tall);
        printer.flush_newline();
        printer.into_inner()
    }
}

impl<'a> From<&'a str> for Doc<'a> {
    fn from(value: &'a str) -> Doc<'a> {
        Doc::str(value)
    }
}

impl<'a> From<String> for Doc<'a> {
    fn from(value: String) -> Doc<'a> {
        Doc::string(value)
    }
}

impl<'a> std::ops::Add<Doc<'a>> for Doc<'a> {
    type Output = Doc<'a>;

    #[inline]
    fn add(self, that: Doc<'a>) -> Doc<'a> {
        // For the logic, always returning the final catch-all case would
        // suffice, but that would be wasteful and produce a deep tree
        // of small vecs, we can make a shallower tree and avoid some pointer
        // chasing by concatenating. Hopefully LLVM will inline this as well
        // as the call that produced the doc, and then it can statically resolve
        // everything.
        match (self, that) {
            (Doc::Concat(xs), y) if xs.is_empty() => y,
            (x, Doc::Concat(ys)) if ys.is_empty() => x,
            (Doc::Concat(mut xs), Doc::Concat(mut ys)) => {
                xs.append(&mut ys);
                Doc::Concat(xs)
            }
            (Doc::Concat(mut xs), y) => {
                xs.push(y);
                Doc::Concat(xs)
            }
            (x, Doc::Concat(mut ys)) => {
                ys.insert(0, x);
                Doc::Concat(ys)
            }
            (Doc::Indent(x), Doc::Indent(y)) => Doc::Indent(Box::new(*x + *y)),
            (x, y) => Doc::Concat(vec![x, y]),
        }
    }
}

macro_rules! doc_concat {
    { $($fragment:expr)* } => {
        {
            #[allow(unused_mut)]
            let mut result = crate::pprint::Doc::empty();
            $( result = result + $fragment.into(); )*
            result
        }
    }
}
pub(crate) use doc_concat as concat;

macro_rules! group {
    { $($fragment:expr)* } => {
        Doc::Group(Box::new( $crate::pprint::concat! { $($fragment)* } ))
    }
}
pub(crate) use group;

macro_rules! indent {
    { $($fragment:expr)* } => {
        Doc::Indent(Box::new( $crate::pprint::concat! { $($fragment)* } ))
    }
}
pub(crate) use indent;

macro_rules! flush_indent {
    { $($fragment:expr)* } => {
        Doc::FlushIndent(Box::new( $crate::pprint::concat! { $($fragment)* } ))
    }
}
pub(crate) use flush_indent;

/// Helper module for pretty printing.
///
/// This is a separate module to be able to hide some of the printer internals
/// from the [`Doc::println`] implementation.
mod printer {
    use super::{Config, Markup, MarkupMode};

    /// Whether printing in a particular mode fitted or not.
    ///
    /// The `Ord` impl returns the worst possible result (overflow) as maximum.
    #[derive(Eq, Ord, PartialEq, PartialOrd)]
    pub enum PrintResult {
        /// The content could be printed within the allocated width.
        Fits,

        /// The content exceeded the target width.
        Overflow,
    }

    impl PrintResult {
        pub fn is_overflow(&self) -> bool {
            matches!(self, PrintResult::Overflow)
        }
    }

    /// Helper for pretty-printing documents that tracks indentation state.
    pub struct Printer {
        /// Buffer where we place the output.
        out: String,

        /// Target width that we should try to not exceed.
        width: u32,

        /// The width so far of the line that we are currently writing.
        line_width: u32,

        /// The current indentation level, counted in spaces.
        indent: u32,

        /// Whether indentation has been written for the current line.
        needs_indent: bool,

        /// The currently applied markup.
        markup: Option<Markup>,

        /// How to apply markup.
        markup_mode: MarkupMode,
    }

    impl Printer {
        /// Create a new printer with the given line width target.
        pub fn new(config: &Config) -> Printer {
            Printer {
                out: String::new(),
                width: config.width,
                line_width: 0,
                indent: 0,
                needs_indent: true,
                markup: None,
                markup_mode: config.markup,
            }
        }

        /// Return the result string printed to the printer.
        pub fn into_inner(self) -> String {
            self.out
        }

        /// Execute `f` against this printer. If the result was too wide, roll back.
        pub fn try_<F: FnOnce(&mut Printer) -> PrintResult>(&mut self, f: F) -> PrintResult {
            let len = self.out.len();
            let line_width = self.line_width;
            let needs_indent = self.needs_indent;
            let result = f(self);
            if result.is_overflow() {
                self.out.truncate(len);
                self.line_width = line_width;
                self.needs_indent = needs_indent;
            }
            result
        }

        /// Execute `f` under increased indentation width.
        pub fn indented<F: FnOnce(&mut Printer) -> PrintResult>(&mut self, f: F) -> PrintResult {
            self.indent += 2;
            let result = f(self);
            self.indent -= 2;
            result
        }

        /// Execute `f` with markup applied.
        pub fn with_markup<F: FnOnce(&mut Printer) -> PrintResult>(
            &mut self,
            markup: Markup,
            f: F,
        ) -> PrintResult {
            let prev = self.markup;
            let next = Some(markup);
            let switch_on = self.markup_mode.get_switch(prev, next);
            let switch_off = self.markup_mode.get_switch(next, prev);
            self.out.push_str(switch_on);
            self.markup = next;
            let result = f(self);
            self.markup = prev;
            self.out.push_str(switch_off);
            result
        }

        /// Write the indent after the newline, if needed.
        fn write_indent(&mut self) {
            if !self.needs_indent {
                return;
            }

            // 50 spaces.
            let spaces = "                                                  ";

            let mut n_left = self.indent as usize;
            while n_left > 0 {
                let n = n_left.min(spaces.len());
                self.out.push_str(&spaces[..n]);
                n_left -= n;
            }

            self.line_width += self.indent;
            self.needs_indent = false;
        }

        /// Report whether the current content still fits.
        fn fits(&self) -> PrintResult {
            if self.line_width > self.width {
                PrintResult::Overflow
            } else {
                PrintResult::Fits
            }
        }

        pub fn push_str(&mut self, value: &str, width: u32) -> PrintResult {
            debug_assert!(
                !value.contains('\n'),
                "Use `newline` to push a newline instead."
            );
            self.write_indent();
            self.out.push_str(value);
            self.line_width += width;
            self.fits()
        }

        pub fn push_char(&mut self, ch: char) -> PrintResult {
            debug_assert_ne!(ch, '\n', "Use `newline` to push a newline instead.");
            self.write_indent();
            self.out.push(ch);
            self.line_width += 1;
            self.fits()
        }

        pub fn newline(&mut self) -> PrintResult {
            debug_assert!(
                !self.out.is_empty(),
                "Should not try to create leading whitespace!",
            );

            // HACK: Remove any trailing spaces from the current line before we
            // move on to the next. This is bad because it might trim
            // significant spaces from user code (e.g. a trailing space in
            // Markdown is significant, and maybe you write Markdown in a
            // comment). Or even worse, there may be trailing whitespace in a
            // multiline string literal. But it is the quick and dirty fix for
            // not emitting space after e.g. a multi-line `let` binding. We work
            // around this hack in string literals by escaping trailing spaces,
            // which is arguably better anyway for visibility.
            self.out.truncate(self.out.trim_end_matches(' ').len());

            self.out.push('\n');
            self.line_width = 0;
            self.needs_indent = true;
            // For the print result, we measure until the end of the line, so a
            // newline fits by definition, even if the previous line might have
            // exceeded the target width. This is mostly to simplify call sites
            // where all match arms return `PrintResult`.
            PrintResult::Fits
        }

        /// Emit a newline but without indentation after it.
        pub fn raw_newline(&mut self) -> PrintResult {
            let result = self.newline();
            self.needs_indent = false;
            result
        }

        /// Emit a newline, unless we are still at the start of a line.
        ///
        /// Returns whether the newline was emitted.
        pub fn flush_newline(&mut self) -> bool {
            if self.needs_indent {
                false
            } else {
                self.newline();
                true
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::{Config, Doc, MarkupMode};

    fn print_width(doc: &Doc, width: u32) -> String {
        doc.println(&Config {
            width,
            markup: MarkupMode::None,
        })
    }

    #[test]
    fn format_array_wide_tall() {
        use Doc::{Sep, SoftBreak};
        let doc = group! {
            "["
            SoftBreak
            indent! {
                "elem0" "," Sep
                "elem1" "," Sep
                "elem2" Doc::tall(",")
            }
            SoftBreak
            "]"
        };
        assert_eq!(print_width(&doc, 80), "[elem0, elem1, elem2]\n");
        assert_eq!(print_width(&doc, 5), "[\n  elem0,\n  elem1,\n  elem2,\n]\n");
    }

    #[test]
    fn hard_break_forces_tall_mode() {
        use Doc::{HardBreak, SoftBreak};
        let doc = group! {
            "["
            indent! {
                HardBreak
                "// Comment."
                SoftBreak
                "elem0"
            }
            SoftBreak
            "]"
        };
        // Despite fitting in 80 columns, we should still format tall because of
        // the hard break.
        assert_eq!(print_width(&doc, 80), "[\n  // Comment.\n  elem0\n]\n");
    }

    #[test]
    fn format_wide_in_tall() {
        use Doc::{Sep, SoftBreak};
        let doc = group! {
            "["
            SoftBreak
            indent! {
                group! {
                    "["
                    SoftBreak
                    indent! {
                        "a" "," Sep
                        "b" "," Sep
                        "c" Doc::tall(",")
                    }
                    SoftBreak
                    "]"
                } "," Sep
                "elem0" "," Sep
                "elem1" "," Sep
                "elem2" Doc::tall(",")
            }
            SoftBreak
            "]"
        };
        assert_eq!(print_width(&doc, 80), "[[a, b, c], elem0, elem1, elem2]\n");
        assert_eq!(
            print_width(&doc, 15),
            "[\n  [a, b, c],\n  elem0,\n  elem1,\n  elem2,\n]\n",
        );
        assert_eq!(
            print_width(&doc, 8),
            "[\n  [\n    a,\n    b,\n    c,\n  ],\n  elem0,\n  elem1,\n  elem2,\n]\n",
        );
    }
}
