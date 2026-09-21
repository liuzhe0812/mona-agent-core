use agent_api::{AgentError, ErrorCode, Result};

/// Incremental SSE decoding at byte boundaries. UTF-8 decoding happens per full line.
pub(crate) struct SseDecoder {
    pending: Vec<u8>, data: Vec<String>, frame_bytes: usize, max_frame: usize, first_line: bool,
}
impl SseDecoder {
    pub fn new(max_frame: usize) -> Self {
        Self { pending: vec![], data: vec![], frame_bytes: 0, max_frame, first_line: true }
    }
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<String>> {
        self.pending.extend_from_slice(bytes);
        let mut events = vec![];
        let mut consumed = 0;
        while let Some(relative) = self.pending[consumed..].iter().position(|b| *b == b'\n') {
            let end = consumed + relative;
            let raw = &self.pending[consumed..end];
            self.frame_bytes = self.frame_bytes.saturating_add(raw.len() + 1);
            if self.frame_bytes > self.max_frame {
                return Err(AgentError::new(ErrorCode::Limit, "SSE event exceeds configured byte limit"));
            }
            let decoded = std::str::from_utf8(raw)
                .map_err(|_| AgentError::new(ErrorCode::ModelProtocol, "SSE is not UTF-8"))?;
            let mut line = decoded.strip_suffix('\r').unwrap_or(decoded);
            if self.first_line { line = line.strip_prefix('\u{feff}').unwrap_or(line); self.first_line = false; }
            if line.is_empty() {
                if !self.data.is_empty() { events.push(self.data.join("\n")); self.data.clear(); }
                self.frame_bytes = 0;
            } else if line == "data" { self.data.push(String::new()); }
            else if let Some(data) = line.strip_prefix("data:") { self.data.push(data.strip_prefix(' ').unwrap_or(data).to_owned()); }
            consumed = end + 1;
        }
        self.pending.drain(..consumed);
        if self.pending.len().saturating_add(self.frame_bytes) > self.max_frame {
            return Err(AgentError::new(ErrorCode::Limit, "unterminated SSE event exceeds byte limit"));
        }
        Ok(events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unicode_and_crlf_survive_arbitrary_network_splits() {
        let source = "\u{feff}: comment\r\ndata: 你好\r\ndata: 世界\r\n\r\ndata: [DONE]\r\n\r\n";
        for split in 0..=source.len() {
            let mut decoder = SseDecoder::new(4096);
            let mut events = decoder.push(&source.as_bytes()[..split]).unwrap();
            events.extend(decoder.push(&source.as_bytes()[split..]).unwrap());
            assert_eq!(events, vec!["你好\n世界", "[DONE]"]);
        }
    }
    #[test]
    fn one_byte_at_a_time() {
        let mut decoder = SseDecoder::new(1024);
        let mut events = vec![];
        for byte in b"data: one\n\ndata: two\n\n" { events.extend(decoder.push(&[*byte]).unwrap()); }
        assert_eq!(events, vec!["one", "two"]);
    }
    #[test]
    fn large_unterminated_frame_is_rejected() {
        let mut decoder = SseDecoder::new(16);
        assert_eq!(decoder.push(b"data: aaaaaaaaaaaaaaaaaaaaaaaaa").unwrap_err().code, ErrorCode::Limit);
    }
    #[test]
    fn invalid_utf8_is_rejected() {
        assert!(SseDecoder::new(128).push(b"data: \xff\n\n").is_err());
    }
}
