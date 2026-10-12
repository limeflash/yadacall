//! Small incremental Server-Sent Events decoder shared by both HTTP providers.

/// An SSE event's data and optional event name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SseEvent {
    /// Joined `data:` fields.
    pub data: String,
    /// Optional `event:` field.
    pub event: Option<String>,
}

/// Incremental SSE line decoder. It buffers bytes so UTF-8 code points may cross HTTP chunks.
#[derive(Debug, Default)]
pub(crate) struct SseParser {
    line: Vec<u8>,
    data: Vec<String>,
    event: Option<String>,
}

impl SseParser {
    /// Feed bytes from an HTTP body and return every complete event.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<SseEvent> {
        let mut events = Vec::new();
        for byte in bytes {
            if *byte == b'\n' {
                if self.line.last() == Some(&b'\r') {
                    self.line.pop();
                }
                self.consume_line(&mut events);
            } else {
                self.line.push(*byte);
            }
        }
        events
    }

    /// Flush a final unterminated line and event at end of stream.
    pub fn finish(&mut self) -> Vec<SseEvent> {
        let mut events = Vec::new();
        if !self.line.is_empty() {
            self.consume_line(&mut events);
        }
        self.dispatch(&mut events);
        events
    }

    fn consume_line(&mut self, events: &mut Vec<SseEvent>) {
        if self.line.is_empty() {
            self.dispatch(events);
            return;
        }
        if self.line.first() == Some(&b':') {
            self.line.clear();
            return;
        }
        let separator = self.line.iter().position(|byte| *byte == b':');
        let (field, mut value) = match separator {
            Some(index) => (&self.line[..index], &self.line[index + 1..]),
            None => (&self.line[..], &[][..]),
        };
        if value.first() == Some(&b' ') {
            value = &value[1..];
        }
        if let Ok(value) = std::str::from_utf8(value) {
            match field {
                b"data" => self.data.push(value.to_owned()),
                b"event" => self.event = Some(value.to_owned()),
                _ => {}
            }
        }
        self.line.clear();
    }

    fn dispatch(&mut self, events: &mut Vec<SseEvent>) {
        if !self.data.is_empty() || self.event.is_some() {
            events.push(SseEvent {
                data: self.data.join("\n"),
                event: self.event.take(),
            });
            self.data.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SseParser;

    #[test]
    fn handles_crlf_split_fields_and_multibyte_utf8() {
        let mut parser = SseParser::default();
        let bytes = "event: message\r\ndata: Привёт 🌍\r\n\r\n".as_bytes();
        let split = bytes.iter().position(|byte| *byte == 0xd0).unwrap_or(1) + 1;
        let mut events = parser.push(&bytes[..split]);
        events.extend(parser.push(&bytes[split..]));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event.as_deref(), Some("message"));
        assert_eq!(events[0].data, "Привёт 🌍");
    }

    #[test]
    fn joins_multiple_data_lines_and_ignores_comments() {
        let mut parser = SseParser::default();
        let events = parser.push(b": ping\ndata: one\ndata: two\n\n");
        assert_eq!(events[0].data, "one\ntwo");
    }
}
