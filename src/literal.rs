use memchr::memmem;
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RewriteError {
    #[error("rewrite pipeline must contain at least one rule")]
    EmptyRules,
    #[error("rewrite source must not be empty")]
    EmptySource,
    #[error("rewrite replacement must end with the source literal")]
    InvalidReplacement,
    #[error("decoded response exceeds the configured {limit} byte limit")]
    TooLarge { limit: usize },
    #[error("response text is not valid UTF-8")]
    InvalidUtf8,
    #[error("rewrite stream has already finished")]
    AlreadyFinished,
}

#[derive(Debug)]
pub struct StreamingRewritePipeline {
    rewriters: Vec<StreamingRewriter>,
    max_bytes: usize,
    total_bytes: usize,
    finished: bool,
}

#[derive(Debug)]
enum StreamingRewriter {
    Literal(StreamingLiteralRewriter),
    ExpressionPattern(StreamingExpressionPatternRewriter),
}

impl StreamingRewriter {
    fn push(&mut self, input: &[u8]) -> Result<Vec<u8>, RewriteError> {
        match self {
            Self::Literal(rewriter) => rewriter.push(input),
            Self::ExpressionPattern(rewriter) => rewriter.push(input),
        }
    }

    fn finish(&mut self) -> Result<Vec<u8>, RewriteError> {
        match self {
            Self::Literal(rewriter) => rewriter.finish(),
            Self::ExpressionPattern(rewriter) => rewriter.finish(),
        }
    }
}

impl StreamingRewritePipeline {
    pub fn new<I, S, R>(rules: I, max_bytes: usize) -> Result<Self, RewriteError>
    where
        I: IntoIterator<Item = (S, R)>,
        S: AsRef<[u8]>,
        R: AsRef<[u8]>,
    {
        let rewriters = rules
            .into_iter()
            .map(|(source, replacement)| {
                StreamingLiteralRewriter::new(source, replacement, usize::MAX)
                    .map(StreamingRewriter::Literal)
            })
            .collect::<Result<Vec<_>, _>>()?;
        Self::from_rewriters(rewriters, max_bytes)
    }

    pub fn new_with_exact<PI, PS, PR, EI, ES, ER>(
        prefix_rules: PI,
        exact_rules: EI,
        max_bytes: usize,
    ) -> Result<Self, RewriteError>
    where
        PI: IntoIterator<Item = (PS, PR)>,
        PS: AsRef<[u8]>,
        PR: AsRef<[u8]>,
        EI: IntoIterator<Item = (ES, ER)>,
        ES: AsRef<[u8]>,
        ER: AsRef<[u8]>,
    {
        let mut rewriters = prefix_rules
            .into_iter()
            .map(|(source, replacement)| {
                StreamingLiteralRewriter::new(source, replacement, usize::MAX)
                    .map(StreamingRewriter::Literal)
            })
            .collect::<Result<Vec<_>, _>>()?;
        rewriters.extend(
            exact_rules
                .into_iter()
                .map(|(source, replacement)| {
                    StreamingLiteralRewriter::new_exact(source, replacement, usize::MAX)
                        .map(StreamingRewriter::Literal)
                })
                .collect::<Result<Vec<_>, _>>()?,
        );
        Self::from_rewriters(rewriters, max_bytes)
    }

    pub(crate) fn new_with_exact_and_member_expression_patterns<
        PI,
        PS,
        PR,
        EI,
        ES,
        ER,
        II,
        IP,
        IS,
        IR,
    >(
        prefix_rules: PI,
        exact_rules: EI,
        member_expression_rules: II,
        max_bytes: usize,
    ) -> Result<Self, RewriteError>
    where
        PI: IntoIterator<Item = (PS, PR)>,
        PS: AsRef<[u8]>,
        PR: AsRef<[u8]>,
        EI: IntoIterator<Item = (ES, ER)>,
        ES: AsRef<[u8]>,
        ER: AsRef<[u8]>,
        II: IntoIterator<Item = (IP, IS, IR)>,
        IP: AsRef<[u8]>,
        IS: AsRef<[u8]>,
        IR: AsRef<[u8]>,
    {
        let mut pipeline = Self::new_with_exact(prefix_rules, exact_rules, max_bytes)?;
        pipeline.rewriters.extend(
            member_expression_rules
                .into_iter()
                .map(|(prefix, suffix, replacement_prefix)| {
                    StreamingExpressionPatternRewriter::new_member_expression(
                        prefix,
                        suffix,
                        replacement_prefix,
                    )
                    .map(StreamingRewriter::ExpressionPattern)
                })
                .collect::<Result<Vec<_>, _>>()?,
        );
        Ok(pipeline)
    }

    pub(crate) fn new_with_appended_suffix(
        source: impl AsRef<[u8]>,
        inserted_suffix: impl AsRef<[u8]>,
        max_bytes: usize,
    ) -> Result<Self, RewriteError> {
        let rewriter = StreamingLiteralRewriter::new_with_appended_suffix(
            source,
            inserted_suffix,
            usize::MAX,
        )?;
        Self::from_rewriters(vec![StreamingRewriter::Literal(rewriter)], max_bytes)
    }

    pub(crate) fn with_identifier_template_patterns<II, IP, IS, IT>(
        mut self,
        identifier_rules: II,
    ) -> Result<Self, RewriteError>
    where
        II: IntoIterator<Item = (IP, IS, IT)>,
        IP: AsRef<[u8]>,
        IS: AsRef<[u8]>,
        IT: AsRef<[u8]>,
    {
        self.rewriters.extend(
            identifier_rules
                .into_iter()
                .map(|(prefix, suffix, template)| {
                    StreamingExpressionPatternRewriter::new_template(prefix, suffix, template)
                        .map(StreamingRewriter::ExpressionPattern)
                })
                .collect::<Result<Vec<_>, _>>()?,
        );
        Ok(self)
    }

    fn from_rewriters(
        rewriters: Vec<StreamingRewriter>,
        max_bytes: usize,
    ) -> Result<Self, RewriteError> {
        if rewriters.is_empty() {
            return Err(RewriteError::EmptyRules);
        }
        Ok(Self {
            rewriters,
            max_bytes,
            total_bytes: 0,
            finished: false,
        })
    }

    pub fn push(&mut self, input: &[u8]) -> Result<Vec<u8>, RewriteError> {
        if self.finished {
            return Err(RewriteError::AlreadyFinished);
        }
        self.total_bytes =
            self.total_bytes
                .checked_add(input.len())
                .ok_or(RewriteError::TooLarge {
                    limit: self.max_bytes,
                })?;
        if self.total_bytes > self.max_bytes {
            return Err(RewriteError::TooLarge {
                limit: self.max_bytes,
            });
        }

        let (first, remaining) = self
            .rewriters
            .split_first_mut()
            .expect("a rewrite pipeline always contains at least one rule");
        let mut output = first.push(input)?;
        for rewriter in remaining {
            output = rewriter.push(&output)?;
        }
        Ok(output)
    }

    pub fn finish(&mut self) -> Result<Vec<u8>, RewriteError> {
        if self.finished {
            return Err(RewriteError::AlreadyFinished);
        }
        self.finished = true;

        let mut output = self.rewriters[0].finish()?;
        for rewriter in &mut self.rewriters[1..] {
            let mut next = rewriter.push(&output)?;
            next.extend(rewriter.finish()?);
            output = next;
        }
        Ok(output)
    }
}

#[derive(Debug)]
struct StreamingExpressionPatternRewriter {
    prefix_finder: memmem::Finder<'static>,
    suffix: Vec<u8>,
    replacement: ExpressionReplacement,
    syntax: PatternSyntax,
    pending: Vec<u8>,
    previous_byte: Option<u8>,
    finished: bool,
}

#[derive(Debug, Clone, Copy)]
enum PatternSyntax {
    Identifier,
    MemberExpression,
}

#[derive(Debug)]
enum ExpressionReplacement {
    Prefix(Vec<u8>),
    Template(Vec<u8>),
}

#[derive(Debug, PartialEq, Eq)]
enum ExpressionPatternMatch {
    Match(usize),
    NeedMore,
    NoMatch,
}

impl StreamingExpressionPatternRewriter {
    fn new(
        prefix: impl AsRef<[u8]>,
        suffix: impl AsRef<[u8]>,
        replacement_prefix: impl AsRef<[u8]>,
    ) -> Result<Self, RewriteError> {
        let prefix = prefix.as_ref();
        let suffix = suffix.as_ref();
        if prefix.is_empty() || suffix.is_empty() {
            return Err(RewriteError::EmptySource);
        }
        Ok(Self {
            prefix_finder: memmem::Finder::new(prefix).into_owned(),
            suffix: suffix.to_vec(),
            replacement: ExpressionReplacement::Prefix(replacement_prefix.as_ref().to_vec()),
            syntax: PatternSyntax::Identifier,
            pending: Vec::new(),
            previous_byte: None,
            finished: false,
        })
    }

    fn new_member_expression(
        prefix: impl AsRef<[u8]>,
        suffix: impl AsRef<[u8]>,
        replacement_prefix: impl AsRef<[u8]>,
    ) -> Result<Self, RewriteError> {
        let mut rewriter = Self::new(prefix, suffix, replacement_prefix)?;
        rewriter.syntax = PatternSyntax::MemberExpression;
        Ok(rewriter)
    }

    fn new_template(
        prefix: impl AsRef<[u8]>,
        suffix: impl AsRef<[u8]>,
        template: impl AsRef<[u8]>,
    ) -> Result<Self, RewriteError> {
        let prefix = prefix.as_ref();
        let suffix = suffix.as_ref();
        if prefix.is_empty() || suffix.is_empty() {
            return Err(RewriteError::EmptySource);
        }
        Ok(Self {
            prefix_finder: memmem::Finder::new(prefix).into_owned(),
            suffix: suffix.to_vec(),
            replacement: ExpressionReplacement::Template(template.as_ref().to_vec()),
            syntax: PatternSyntax::Identifier,
            pending: Vec::new(),
            previous_byte: None,
            finished: false,
        })
    }

    fn push(&mut self, input: &[u8]) -> Result<Vec<u8>, RewriteError> {
        if self.finished {
            return Err(RewriteError::AlreadyFinished);
        }
        self.pending.extend_from_slice(input);
        Ok(self.process_available(false))
    }

    fn finish(&mut self) -> Result<Vec<u8>, RewriteError> {
        if self.finished {
            return Err(RewriteError::AlreadyFinished);
        }
        self.finished = true;
        Ok(self.process_available(true))
    }

    fn process_available(&mut self, end_of_stream: bool) -> Vec<u8> {
        let prefix_len = self.prefix_finder.needle().len();
        let mut output = Vec::with_capacity(self.pending.len());
        let mut consumed = 0;
        let mut previous_byte = self.previous_byte;

        loop {
            let Some(relative_position) = self.prefix_finder.find(&self.pending[consumed..]) else {
                let emit_len = if end_of_stream {
                    self.pending.len() - consumed
                } else {
                    (self.pending.len() - consumed).saturating_sub(prefix_len.saturating_sub(1))
                };
                if emit_len > 0 {
                    let emitted = &self.pending[consumed..consumed + emit_len];
                    previous_byte = emitted.last().copied();
                    output.extend_from_slice(emitted);
                    consumed += emit_len;
                }
                break;
            };
            let position = consumed + relative_position;

            if position > consumed {
                let emitted = &self.pending[consumed..position];
                previous_byte = emitted.last().copied();
                output.extend_from_slice(emitted);
                consumed = position;
                continue;
            }

            let preceded_by_identifier = previous_byte.is_some_and(is_ascii_identifier_continue);
            match if preceded_by_identifier {
                ExpressionPatternMatch::NoMatch
            } else {
                self.match_candidate(consumed, end_of_stream)
            } {
                ExpressionPatternMatch::Match(matched_len) => {
                    let expression_start = consumed + prefix_len;
                    let expression_end = consumed + matched_len - self.suffix.len();
                    let expression = &self.pending[expression_start..expression_end];
                    match &self.replacement {
                        ExpressionReplacement::Prefix(replacement_prefix) => {
                            output.extend_from_slice(replacement_prefix);
                            output.extend_from_slice(
                                &self.pending[expression_start..consumed + matched_len],
                            );
                        }
                        ExpressionReplacement::Template(template) => {
                            render_identifier_template(&mut output, template, expression);
                        }
                    }
                    previous_byte = output.last().copied();
                    consumed += matched_len;
                }
                ExpressionPatternMatch::NeedMore => break,
                ExpressionPatternMatch::NoMatch => {
                    output.push(self.pending[consumed]);
                    previous_byte = Some(self.pending[consumed]);
                    consumed += 1;
                }
            }
        }
        self.previous_byte = previous_byte;
        self.pending.drain(..consumed);
        output
    }

    fn match_candidate(&self, start: usize, end_of_stream: bool) -> ExpressionPatternMatch {
        let mut cursor = start + self.prefix_finder.needle().len();
        cursor = match scan_identifier_segment(&self.pending, cursor, end_of_stream) {
            IdentifierSegmentScan::End(cursor) => cursor,
            IdentifierSegmentScan::NeedMore => return ExpressionPatternMatch::NeedMore,
            IdentifierSegmentScan::NoMatch => return ExpressionPatternMatch::NoMatch,
        };

        if matches!(self.syntax, PatternSyntax::MemberExpression) {
            while self.pending.get(cursor) == Some(&b'.') {
                cursor += 1;
                cursor = match scan_identifier_segment(&self.pending, cursor, end_of_stream) {
                    IdentifierSegmentScan::End(cursor) => cursor,
                    IdentifierSegmentScan::NeedMore => return ExpressionPatternMatch::NeedMore,
                    IdentifierSegmentScan::NoMatch => return ExpressionPatternMatch::NoMatch,
                };
            }
        }

        if cursor == self.pending.len() {
            return if end_of_stream {
                ExpressionPatternMatch::NoMatch
            } else {
                ExpressionPatternMatch::NeedMore
            };
        }

        let available = &self.pending[cursor..];
        let compared_len = available.len().min(self.suffix.len());
        if available[..compared_len] != self.suffix[..compared_len] {
            return ExpressionPatternMatch::NoMatch;
        }
        if available.len() < self.suffix.len() {
            return if end_of_stream {
                ExpressionPatternMatch::NoMatch
            } else {
                ExpressionPatternMatch::NeedMore
            };
        }
        ExpressionPatternMatch::Match(cursor + self.suffix.len() - start)
    }
}

enum IdentifierSegmentScan {
    End(usize),
    NeedMore,
    NoMatch,
}

fn scan_identifier_segment(
    input: &[u8],
    mut cursor: usize,
    end_of_stream: bool,
) -> IdentifierSegmentScan {
    let Some(&first) = input.get(cursor) else {
        return if end_of_stream {
            IdentifierSegmentScan::NoMatch
        } else {
            IdentifierSegmentScan::NeedMore
        };
    };
    if !is_ascii_identifier_start(first) {
        return IdentifierSegmentScan::NoMatch;
    }
    cursor += 1;
    while input
        .get(cursor)
        .is_some_and(|byte| is_ascii_identifier_continue(*byte))
    {
        cursor += 1;
    }
    IdentifierSegmentScan::End(cursor)
}

fn render_identifier_template(output: &mut Vec<u8>, template: &[u8], expression: &[u8]) {
    const MARKER: &[u8] = b"{identifier}";
    let finder = memmem::Finder::new(MARKER);
    let mut consumed = 0;
    for position in finder.find_iter(template) {
        output.extend_from_slice(&template[consumed..position]);
        output.extend_from_slice(expression);
        consumed = position + MARKER.len();
    }
    output.extend_from_slice(&template[consumed..]);
}

// Webpack and Terser emit ASCII binding identifiers; keep this matcher scoped to that output.
fn is_ascii_identifier_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || matches!(byte, b'_' | b'$')
}

fn is_ascii_identifier_continue(byte: u8) -> bool {
    is_ascii_identifier_start(byte) || byte.is_ascii_digit()
}

#[derive(Debug)]
pub struct StreamingLiteralRewriter {
    finder: memmem::Finder<'static>,
    mode: LiteralRewriteMode,
    max_bytes: usize,
    total_bytes: usize,
    pending: Vec<u8>,
    input_history: Vec<u8>,
    utf8_tail: Vec<u8>,
    finished: bool,
}

#[derive(Debug)]
enum LiteralRewriteMode {
    Prepend {
        replacement: Vec<u8>,
        inserted_prefix: Vec<u8>,
    },
    Exact {
        replacement: Vec<u8>,
    },
    Append {
        replacement: Vec<u8>,
        inserted_suffix: Vec<u8>,
    },
}

impl StreamingLiteralRewriter {
    pub fn new(
        source: impl AsRef<[u8]>,
        replacement: impl AsRef<[u8]>,
        max_bytes: usize,
    ) -> Result<Self, RewriteError> {
        let source = source.as_ref();
        let replacement = replacement.as_ref();
        let inserted_prefix = replacement
            .strip_suffix(source)
            .ok_or(RewriteError::InvalidReplacement)?
            .to_vec();

        Self::from_mode(
            source,
            LiteralRewriteMode::Prepend {
                replacement: replacement.to_vec(),
                inserted_prefix,
            },
            max_bytes,
        )
    }

    pub fn new_exact(
        source: impl AsRef<[u8]>,
        replacement: impl AsRef<[u8]>,
        max_bytes: usize,
    ) -> Result<Self, RewriteError> {
        let source = source.as_ref();
        Self::from_mode(
            source,
            LiteralRewriteMode::Exact {
                replacement: replacement.as_ref().to_vec(),
            },
            max_bytes,
        )
    }

    fn new_with_appended_suffix(
        source: impl AsRef<[u8]>,
        inserted_suffix: impl AsRef<[u8]>,
        max_bytes: usize,
    ) -> Result<Self, RewriteError> {
        let source = source.as_ref();
        let inserted_suffix = inserted_suffix.as_ref().to_vec();
        let mut replacement = source.to_vec();
        replacement.extend_from_slice(&inserted_suffix);
        Self::from_mode(
            source,
            LiteralRewriteMode::Append {
                replacement,
                inserted_suffix,
            },
            max_bytes,
        )
    }

    fn from_mode(
        source: &[u8],
        mode: LiteralRewriteMode,
        max_bytes: usize,
    ) -> Result<Self, RewriteError> {
        if source.is_empty() {
            return Err(RewriteError::EmptySource);
        }
        Ok(Self {
            finder: memmem::Finder::new(source).into_owned(),
            mode,
            max_bytes,
            total_bytes: 0,
            pending: Vec::new(),
            input_history: Vec::new(),
            utf8_tail: Vec::new(),
            finished: false,
        })
    }

    pub fn push(&mut self, input: &[u8]) -> Result<Vec<u8>, RewriteError> {
        if self.finished {
            return Err(RewriteError::AlreadyFinished);
        }
        self.total_bytes =
            self.total_bytes
                .checked_add(input.len())
                .ok_or(RewriteError::TooLarge {
                    limit: self.max_bytes,
                })?;
        if self.total_bytes > self.max_bytes {
            return Err(RewriteError::TooLarge {
                limit: self.max_bytes,
            });
        }
        self.validate_utf8(input)?;
        self.pending.extend_from_slice(input);
        Ok(self.process_available(false))
    }

    pub fn finish(&mut self) -> Result<Vec<u8>, RewriteError> {
        if self.finished {
            return Err(RewriteError::AlreadyFinished);
        }
        self.finished = true;
        if !self.utf8_tail.is_empty() {
            return Err(RewriteError::InvalidUtf8);
        }
        Ok(self.process_available(true))
    }

    fn validate_utf8(&mut self, input: &[u8]) -> Result<(), RewriteError> {
        if self.utf8_tail.is_empty() {
            return Self::validate_utf8_slice(input, &mut self.utf8_tail);
        }

        let expected_len = utf8_sequence_len(self.utf8_tail[0]);
        let missing_len = expected_len - self.utf8_tail.len();
        let boundary_input_len = input.len().min(missing_len);
        let mut boundary = [0; 4];
        let tail_len = self.utf8_tail.len();
        boundary[..tail_len].copy_from_slice(&self.utf8_tail);
        boundary[tail_len..tail_len + boundary_input_len]
            .copy_from_slice(&input[..boundary_input_len]);
        let boundary_len = tail_len + boundary_input_len;

        match std::str::from_utf8(&boundary[..boundary_len]) {
            Ok(_) => {
                self.utf8_tail.clear();
                Self::validate_utf8_slice(&input[boundary_input_len..], &mut self.utf8_tail)
            }
            Err(error) if error.error_len().is_none() => {
                self.utf8_tail
                    .extend_from_slice(&input[..boundary_input_len]);
                Ok(())
            }
            Err(_) => {
                self.utf8_tail.clear();
                Err(RewriteError::InvalidUtf8)
            }
        }
    }

    fn validate_utf8_slice(input: &[u8], utf8_tail: &mut Vec<u8>) -> Result<(), RewriteError> {
        match std::str::from_utf8(input) {
            Ok(_) => Ok(()),
            Err(error) if error.error_len().is_none() => {
                utf8_tail.extend_from_slice(&input[error.valid_up_to()..]);
                Ok(())
            }
            Err(_) => Err(RewriteError::InvalidUtf8),
        }
    }

    fn process_available(&mut self, end_of_stream: bool) -> Vec<u8> {
        let source = self.finder.needle();
        let source_len = source.len();
        let (replacement, inserted_prefix, inserted_suffix) = match &self.mode {
            LiteralRewriteMode::Prepend {
                replacement,
                inserted_prefix,
            } => (
                replacement.as_slice(),
                Some(inserted_prefix.as_slice()),
                None,
            ),
            LiteralRewriteMode::Exact { replacement } => (replacement.as_slice(), None, None),
            LiteralRewriteMode::Append {
                replacement,
                inserted_suffix,
            } => (
                replacement.as_slice(),
                None,
                Some(inserted_suffix.as_slice()),
            ),
        };
        let mut input_history = std::mem::take(&mut self.input_history);
        let mut output = Vec::with_capacity(self.pending.len());
        let mut consumed = 0;

        while let Some(relative_position) = self.finder.find(&self.pending[consumed..]) {
            let position = consumed + relative_position;
            let before_match = &self.pending[consumed..position];
            let already_prefixed =
                original_input_ends_with_prefix(inserted_prefix, &input_history, before_match);
            output.extend_from_slice(before_match);
            remember_input(inserted_prefix, &mut input_history, before_match);
            let after_match = position + source_len;
            if let Some(inserted_suffix) = inserted_suffix {
                let available_after = &self.pending[after_match..];
                if !end_of_stream
                    && available_after.len() < inserted_suffix.len()
                    && inserted_suffix.starts_with(available_after)
                {
                    self.input_history = input_history;
                    self.pending.drain(..position);
                    return output;
                }
                if available_after.starts_with(inserted_suffix) {
                    output.extend_from_slice(source);
                } else {
                    output.extend_from_slice(replacement);
                }
            } else if already_prefixed {
                output.extend_from_slice(source);
            } else {
                output.extend_from_slice(replacement);
            }
            remember_input(inserted_prefix, &mut input_history, source);
            consumed = position + source_len;
        }

        let emit_len = if end_of_stream {
            self.pending.len() - consumed
        } else {
            (self.pending.len() - consumed).saturating_sub(source_len.saturating_sub(1))
        };
        if emit_len > 0 {
            let emitted = &self.pending[consumed..consumed + emit_len];
            output.extend_from_slice(emitted);
            remember_input(inserted_prefix, &mut input_history, emitted);
            consumed += emit_len;
        }
        self.input_history = input_history;
        self.pending.drain(..consumed);
        output
    }
}

fn utf8_sequence_len(first_byte: u8) -> usize {
    match first_byte {
        0xC2..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF4 => 4,
        _ => unreachable!("an incomplete UTF-8 tail always starts with a valid leading byte"),
    }
}

fn original_input_ends_with_prefix(
    inserted_prefix: Option<&[u8]>,
    input_history: &[u8],
    before_match: &[u8],
) -> bool {
    let Some(inserted_prefix) = inserted_prefix else {
        return false;
    };
    if inserted_prefix.is_empty() {
        return true;
    }
    if before_match.len() >= inserted_prefix.len() {
        return before_match.ends_with(inserted_prefix);
    }

    let missing = inserted_prefix.len() - before_match.len();
    input_history.len() >= missing
        && input_history[input_history.len() - missing..] == inserted_prefix[..missing]
        && before_match == &inserted_prefix[missing..]
}

fn remember_input(inserted_prefix: Option<&[u8]>, input_history: &mut Vec<u8>, input: &[u8]) {
    let Some(inserted_prefix) = inserted_prefix else {
        return;
    };
    let history_len = inserted_prefix.len();
    if history_len == 0 {
        return;
    }
    if input.len() >= history_len {
        input_history.clear();
        input_history.extend_from_slice(&input[input.len() - history_len..]);
        return;
    }

    let remove = (input_history.len() + input.len()).saturating_sub(history_len);
    if remove > 0 {
        input_history.copy_within(remove.., 0);
        input_history.truncate(input_history.len() - remove);
    }
    input_history.extend_from_slice(input);
}
