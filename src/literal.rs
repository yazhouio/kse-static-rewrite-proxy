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
    IdentifierPattern(StreamingIdentifierPatternRewriter),
}

impl StreamingRewriter {
    fn push(&mut self, input: &[u8]) -> Result<Vec<u8>, RewriteError> {
        match self {
            Self::Literal(rewriter) => rewriter.push(input),
            Self::IdentifierPattern(rewriter) => rewriter.push(input),
        }
    }

    fn finish(&mut self) -> Result<Vec<u8>, RewriteError> {
        match self {
            Self::Literal(rewriter) => rewriter.finish(),
            Self::IdentifierPattern(rewriter) => rewriter.finish(),
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

    pub(crate) fn new_with_exact_and_identifier_patterns<PI, PS, PR, EI, ES, ER, II, IP, IS, IR>(
        prefix_rules: PI,
        exact_rules: EI,
        identifier_rules: II,
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
            identifier_rules
                .into_iter()
                .map(|(prefix, suffix, replacement_prefix)| {
                    StreamingIdentifierPatternRewriter::new(prefix, suffix, replacement_prefix)
                        .map(StreamingRewriter::IdentifierPattern)
                })
                .collect::<Result<Vec<_>, _>>()?,
        );
        Ok(pipeline)
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

        let mut output = input.to_vec();
        for rewriter in &mut self.rewriters {
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
struct StreamingIdentifierPatternRewriter {
    prefix: Vec<u8>,
    suffix: Vec<u8>,
    replacement_prefix: Vec<u8>,
    pending: Vec<u8>,
    previous_byte: Option<u8>,
    finished: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum IdentifierPatternMatch {
    Match(usize),
    NeedMore,
    NoMatch,
}

impl StreamingIdentifierPatternRewriter {
    fn new(
        prefix: impl AsRef<[u8]>,
        suffix: impl AsRef<[u8]>,
        replacement_prefix: impl AsRef<[u8]>,
    ) -> Result<Self, RewriteError> {
        let prefix = prefix.as_ref().to_vec();
        let suffix = suffix.as_ref().to_vec();
        if prefix.is_empty() || suffix.is_empty() {
            return Err(RewriteError::EmptySource);
        }
        Ok(Self {
            prefix,
            suffix,
            replacement_prefix: replacement_prefix.as_ref().to_vec(),
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
        let mut output = Vec::new();
        loop {
            let Some(position) = memmem::find(&self.pending, &self.prefix) else {
                let emit_len = if end_of_stream {
                    self.pending.len()
                } else {
                    self.pending
                        .len()
                        .saturating_sub(self.prefix.len().saturating_sub(1))
                };
                self.emit_pending(&mut output, emit_len);
                break;
            };

            if position > 0 {
                self.emit_pending(&mut output, position);
                continue;
            }

            let preceded_by_identifier =
                self.previous_byte.is_some_and(is_ascii_identifier_continue);
            match if preceded_by_identifier {
                IdentifierPatternMatch::NoMatch
            } else {
                self.match_candidate(end_of_stream)
            } {
                IdentifierPatternMatch::Match(matched_len) => {
                    output.extend_from_slice(&self.replacement_prefix);
                    output.extend_from_slice(&self.pending[self.prefix.len()..matched_len]);
                    self.previous_byte = self.pending.get(matched_len - 1).copied();
                    self.pending.drain(..matched_len);
                }
                IdentifierPatternMatch::NeedMore => break,
                IdentifierPatternMatch::NoMatch => {
                    self.emit_pending(&mut output, 1);
                }
            }
        }
        output
    }

    fn emit_pending(&mut self, output: &mut Vec<u8>, len: usize) {
        if len == 0 {
            return;
        }
        self.previous_byte = self.pending.get(len - 1).copied();
        output.extend(self.pending.drain(..len));
    }

    fn match_candidate(&self, end_of_stream: bool) -> IdentifierPatternMatch {
        let mut cursor = self.prefix.len();
        let Some(&first) = self.pending.get(cursor) else {
            return if end_of_stream {
                IdentifierPatternMatch::NoMatch
            } else {
                IdentifierPatternMatch::NeedMore
            };
        };
        if !is_ascii_identifier_start(first) {
            return IdentifierPatternMatch::NoMatch;
        }
        cursor += 1;

        while let Some(&byte) = self.pending.get(cursor) {
            if !is_ascii_identifier_continue(byte) {
                break;
            }
            cursor += 1;
        }

        if cursor == self.pending.len() {
            return if end_of_stream {
                IdentifierPatternMatch::NoMatch
            } else {
                IdentifierPatternMatch::NeedMore
            };
        }

        let available = &self.pending[cursor..];
        let compared_len = available.len().min(self.suffix.len());
        if available[..compared_len] != self.suffix[..compared_len] {
            return IdentifierPatternMatch::NoMatch;
        }
        if available.len() < self.suffix.len() {
            return if end_of_stream {
                IdentifierPatternMatch::NoMatch
            } else {
                IdentifierPatternMatch::NeedMore
            };
        }
        IdentifierPatternMatch::Match(cursor + self.suffix.len())
    }
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
    source: Vec<u8>,
    replacement: Vec<u8>,
    inserted_prefix: Option<Vec<u8>>,
    max_bytes: usize,
    total_bytes: usize,
    pending: Vec<u8>,
    input_history: Vec<u8>,
    utf8_tail: Vec<u8>,
    finished: bool,
}

impl StreamingLiteralRewriter {
    pub fn new(
        source: impl AsRef<[u8]>,
        replacement: impl AsRef<[u8]>,
        max_bytes: usize,
    ) -> Result<Self, RewriteError> {
        let source = source.as_ref().to_vec();
        let replacement = replacement.as_ref().to_vec();
        if source.is_empty() {
            return Err(RewriteError::EmptySource);
        }
        let inserted_prefix = replacement
            .strip_suffix(source.as_slice())
            .ok_or(RewriteError::InvalidReplacement)?
            .to_vec();

        Ok(Self {
            source,
            replacement,
            inserted_prefix: Some(inserted_prefix),
            max_bytes,
            total_bytes: 0,
            pending: Vec::new(),
            input_history: Vec::new(),
            utf8_tail: Vec::new(),
            finished: false,
        })
    }

    pub fn new_exact(
        source: impl AsRef<[u8]>,
        replacement: impl AsRef<[u8]>,
        max_bytes: usize,
    ) -> Result<Self, RewriteError> {
        let source = source.as_ref().to_vec();
        if source.is_empty() {
            return Err(RewriteError::EmptySource);
        }
        Ok(Self {
            source,
            replacement: replacement.as_ref().to_vec(),
            inserted_prefix: None,
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
        let mut bytes = std::mem::take(&mut self.utf8_tail);
        bytes.extend_from_slice(input);
        match std::str::from_utf8(&bytes) {
            Ok(_) => Ok(()),
            Err(error) if error.error_len().is_none() => {
                self.utf8_tail
                    .extend_from_slice(&bytes[error.valid_up_to()..]);
                Ok(())
            }
            Err(_) => Err(RewriteError::InvalidUtf8),
        }
    }

    fn process_available(&mut self, end_of_stream: bool) -> Vec<u8> {
        let mut output = Vec::new();
        while let Some(position) = memmem::find(&self.pending, &self.source) {
            let before_match = self.pending[..position].to_vec();
            let already_prefixed = self.original_input_ends_with_prefix(&before_match);

            output.extend_from_slice(&before_match);
            self.remember_input(&before_match);
            if already_prefixed {
                output.extend_from_slice(&self.source);
            } else {
                output.extend_from_slice(&self.replacement);
            }
            let source = self.source.clone();
            self.remember_input(&source);
            self.pending.drain(..position + self.source.len());
        }

        let emit_len = if end_of_stream {
            self.pending.len()
        } else {
            self.pending
                .len()
                .saturating_sub(self.source.len().saturating_sub(1))
        };
        if emit_len > 0 {
            let emitted = self.pending[..emit_len].to_vec();
            output.extend_from_slice(&emitted);
            self.remember_input(&emitted);
            self.pending.drain(..emit_len);
        }
        output
    }

    fn original_input_ends_with_prefix(&self, before_match: &[u8]) -> bool {
        let Some(inserted_prefix) = self.inserted_prefix.as_ref() else {
            return false;
        };
        if inserted_prefix.is_empty() {
            return true;
        }
        if before_match.len() >= inserted_prefix.len() {
            return before_match.ends_with(inserted_prefix);
        }

        let missing = inserted_prefix.len() - before_match.len();
        self.input_history.len() >= missing
            && self.input_history[self.input_history.len() - missing..]
                == inserted_prefix[..missing]
            && before_match == &inserted_prefix[missing..]
    }

    fn remember_input(&mut self, input: &[u8]) {
        let Some(inserted_prefix) = self.inserted_prefix.as_ref() else {
            return;
        };
        if inserted_prefix.is_empty() {
            return;
        }
        self.input_history.extend_from_slice(input);
        if self.input_history.len() > inserted_prefix.len() {
            let remove = self.input_history.len() - inserted_prefix.len();
            self.input_history.drain(..remove);
        }
    }
}
