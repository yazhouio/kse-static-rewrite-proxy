use memchr::memmem;
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RewriteError {
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
pub struct StreamingLiteralRewriter {
    source: Vec<u8>,
    replacement: Vec<u8>,
    inserted_prefix: Vec<u8>,
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
            inserted_prefix,
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
        if self.inserted_prefix.is_empty() {
            return true;
        }
        if before_match.len() >= self.inserted_prefix.len() {
            return before_match.ends_with(&self.inserted_prefix);
        }

        let missing = self.inserted_prefix.len() - before_match.len();
        self.input_history.len() >= missing
            && self.input_history[self.input_history.len() - missing..]
                == self.inserted_prefix[..missing]
            && before_match == &self.inserted_prefix[missing..]
    }

    fn remember_input(&mut self, input: &[u8]) {
        if self.inserted_prefix.is_empty() {
            return;
        }
        self.input_history.extend_from_slice(input);
        if self.input_history.len() > self.inserted_prefix.len() {
            let remove = self.input_history.len() - self.inserted_prefix.len();
            self.input_history.drain(..remove);
        }
    }
}
