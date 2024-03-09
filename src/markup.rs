// RCL -- A reasonable configuration language.
// Copyright 2023 Ruud van Asseldonk

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// A copy of the License has been included in the root of the repository.

//! Utilities for dealing with color and other markup.

use std::io::{IsTerminal, Write};

/// A markup hint, used to apply color and other markup to output.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Markup {
    /// No special markup applied, default formatting.
    None,

    /// Used for error message reporting, styled in bold.
    Error,
    /// Used for error message reporting, styled in bold.
    Warning,
    /// Used for trace message reporting, styled in bold.
    Trace,

    /// Make something stand out in error messages.
    ///
    /// We use this to play a similar role as backticks in Markdown,
    /// to clarify visually where the boundaries of a quotation are.
    Highlight,

    // These are meant for syntax highlighting.
    Builtin,
    Comment,
    Identifier,
    Keyword,
    Number,
    String,
    Type,
}

/// How to treat color and other markup hints.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MarkupMode {
    /// Ignore all markup hints, do not output them.
    None,
    /// Output markup as ANSI escape sequences.
    Ansi,
}

/// Whether we should use ANSI colors when writing to this file descriptor.
///
/// Returns true when the file descriptor refers to a terminal, unless the
/// `NO_COLOR` environment variable is set to a nonempty string. See also
/// <https://no-color.org/>.
fn should_color<T: IsTerminal>(fd: &T) -> bool {
    if !fd.is_terminal() {
        return false;
    }
    // coverage:off -- Tests never run with a terminal, so this is never covered.
    match std::env::var("NO_COLOR") {
        Ok(no_color) => no_color == "",
        Err(..) => true,
    }
    // coverage:on
}

impl MarkupMode {
    /// Get the default markup configuration for a file descriptor.
    pub fn default_for_fd<T: IsTerminal>(fd: &T) -> Self {
        if should_color(fd) {
            MarkupMode::Ansi
        } else {
            MarkupMode::None
        }
    }
}

/// Return the ANSI escape code to switch to style `markup`.
pub fn switch_ansi(markup: Markup) -> &'static str {
    let reset = "\x1b[0m";
    let bold_red = "\x1b[31;1m";
    let bold_yellow = "\x1b[33;1m";
    let bold_blue = "\x1b[34;1m";
    let red = "\x1b[31m";
    let green = "\x1b[32m";
    let blue = "\x1b[34m";
    let magenta = "\x1b[35m";
    let cyan = "\x1b[36m";
    let white = "\x1b[37m";

    match markup {
        Markup::None => reset,
        Markup::Error => bold_red,
        Markup::Warning => bold_yellow,
        Markup::Trace => bold_blue,
        Markup::Highlight => white,
        Markup::Builtin => red,
        Markup::Comment => white,
        Markup::Identifier => blue,
        Markup::Keyword => green,
        Markup::Number => cyan,
        Markup::String => red,
        Markup::Type => magenta,
    }
}

/// A string pieced together from fragments that have markup.
pub struct MarkupString<'a> {
    fragments: Vec<(&'a str, Markup)>,
    // TODO: We could keep track of the length, then to_string could preallocate
    // a buffer of the right size.
}

impl<'a> MarkupString<'a> {
    pub fn new() -> MarkupString<'a> {
        MarkupString {
            fragments: Vec::new(),
        }
    }

    /// Append a new fragment.
    pub fn push(&mut self, fragment: &'a str, markup: Markup) {
        debug_assert!(!fragment.is_empty(), "Should not push empty fragments.");
        self.fragments.push((fragment, markup));
    }

    /// Return the current number of fragments.
    pub fn num_fragments(&self) -> usize {
        self.fragments.len()
    }

    /// Return whether the string is empty.
    pub fn is_empty(&self) -> bool {
        self.fragments.is_empty()
    }

    /// Drop fragments, restore to the given previous length.
    pub fn truncate(&mut self, num_fragments: usize) {
        self.fragments.truncate(num_fragments)
    }

    /// Remove all spaces at the end.
    pub fn trim_spaces_end(&mut self) {
        while let Some((fragment, _markup)) = self.fragments.last_mut() {
            let f_trimmed = fragment.trim_end_matches(' ');
            if f_trimmed.is_empty() {
                self.fragments.pop();
            } else {
                *fragment = f_trimmed;
                break;
            }
        }
    }

    /// Append the string to a regular `String`, discarding all markup.
    #[inline]
    pub fn write_string_no_markup(&self, out: &mut String) {
        for (frag_str, _markup) in self.fragments.iter() {
            out.push_str(frag_str);
        }
    }

    /// Append the string to a regular `String`, discarding all markup.
    pub fn to_string_no_markup(&self) -> String {
        let mut out = String::new();
        self.write_string_no_markup(&mut out);
        out
    }

    /// Write the string to a writer, discarding all markup.
    pub fn write_bytes_no_markup(&self, out: &mut dyn Write) -> std::io::Result<()> {
        for (frag_str, _markup) in self.fragments.iter() {
            out.write_all(frag_str.as_bytes())?
        }
        Ok(())
    }

    /// Write the string to a writer, using ANSI escape codes for markup.
    pub fn write_bytes_ansi(&self, out: &mut dyn Write) -> std::io::Result<()> {
        let mut markup = Markup::None;

        for (frag_str, frag_markup) in self.fragments.iter() {
            if markup != *frag_markup {
                out.write_all(switch_ansi(*frag_markup).as_bytes())?;
                markup = *frag_markup;
            }
            out.write_all(frag_str.as_bytes())?;
        }

        Ok(())
    }

    /// Write the string to a write with the given markup mode.
    pub fn write_bytes(&self, mode: MarkupMode, out: &mut dyn Write) -> std::io::Result<()> {
        match mode {
            MarkupMode::None => self.write_bytes_no_markup(out),
            MarkupMode::Ansi => self.write_bytes_ansi(out),
        }
    }
}
