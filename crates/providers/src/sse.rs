//! Bounded Server-Sent Events framing decoder.

use crate::ProviderError;
use std::str;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SseEvent {
    pub event: Option<String>,
    pub id: Option<String>,
    pub data: String,
}

pub struct SseDecoder {
    buffer: Vec<u8>,
    event: Option<String>,
    id: Option<String>,
    data: Vec<String>,
    max_event_bytes: usize,
    finished: bool,
}

impl SseDecoder {
    pub fn new(max_event_bytes: usize) -> Result<Self, ProviderError> {
        if max_event_bytes == 0 {
            return Err(ProviderError::InvalidConfiguration(
                "SSE event limit is zero",
            ));
        }
        Ok(Self {
            buffer: Vec::new(),
            event: None,
            id: None,
            data: Vec::new(),
            max_event_bytes,
            finished: false,
        })
    }

    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<SseEvent>, ProviderError> {
        if self.finished {
            return Err(ProviderError::InvalidResponse("SSE data after DONE"));
        }
        self.buffer.extend_from_slice(bytes);
        let mut events = Vec::new();
        while let Some((end, newline_len)) = find_line_end(&self.buffer) {
            let mut line = self.buffer.drain(..end).collect::<Vec<_>>();
            line.truncate(line.len() - newline_len);
            self.consume_line(&line, &mut events)?;
        }
        if self.buffer.len() > self.max_event_bytes {
            return Err(ProviderError::ResponseTooLarge);
        }
        Ok(events)
    }

    pub fn finish(&mut self) -> Result<Vec<SseEvent>, ProviderError> {
        if self.finished {
            return Ok(Vec::new());
        }
        let mut events = Vec::new();
        if !self.buffer.is_empty() {
            let line = std::mem::take(&mut self.buffer);
            self.consume_line(&line, &mut events)?;
        }
        if !self.data.is_empty() {
            events.extend(self.dispatch()?);
        }
        Ok(events)
    }

    fn consume_line(
        &mut self,
        line: &[u8],
        events: &mut Vec<SseEvent>,
    ) -> Result<(), ProviderError> {
        if self.finished && !line.is_empty() {
            return Err(ProviderError::InvalidResponse("SSE data after DONE"));
        }
        if line.is_empty() {
            if !self.data.is_empty() {
                events.extend(self.dispatch()?);
            }
            return Ok(());
        }
        if line[0] == b':' {
            return Ok(());
        }
        let (field, value) = match line.iter().position(|b| *b == b':') {
            Some(i) => (
                &line[..i],
                line[i + 1..].strip_prefix(b" ").unwrap_or(&line[i + 1..]),
            ),
            None => (line, &[][..]),
        };
        let field = str::from_utf8(field)
            .map_err(|_| ProviderError::InvalidResponse("SSE field is not UTF-8"))?;
        let value = str::from_utf8(value)
            .map_err(|_| ProviderError::InvalidResponse("SSE value is not UTF-8"))?
            .to_owned();
        match field {
            "event" => self.event = Some(value),
            "id" => self.id = Some(value),
            "data" => {
                self.data.push(value);
                if self.data.iter().map(String::len).sum::<usize>() > self.max_event_bytes {
                    return Err(ProviderError::ResponseTooLarge);
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn dispatch(&mut self) -> Result<Vec<SseEvent>, ProviderError> {
        let event = SseEvent {
            event: self.event.take(),
            id: self.id.take(),
            data: self.data.drain(..).collect::<Vec<_>>().join("\n"),
        };
        if event.data == "[DONE]" {
            self.finished = true;
        }
        Ok(vec![event])
    }
}

fn find_line_end(buffer: &[u8]) -> Option<(usize, usize)> {
    for (index, byte) in buffer.iter().enumerate() {
        if *byte == b'\n' {
            return Some((index + 1, 1));
        }
        if *byte == b'\r' {
            if buffer.get(index + 1) == Some(&b'\n') {
                return Some((index + 2, 2));
            }
            if index + 1 < buffer.len() {
                return Some((index + 1, 1));
            }
            return None;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_split_utf8_crlf_comments_and_multiple_data() {
        let mut d = SseDecoder::new(1024).unwrap();
        assert!(d.push(b": heartbeat\r\n").unwrap().is_empty());
        assert!(
            d.push("event: message\r\ndata: {\"x\":\"你".as_bytes())
                .unwrap()
                .is_empty()
        );
        let e = d
            .push("好\"}\r\ndata: {\"y\":1}\r\n\r\n".as_bytes())
            .unwrap();
        assert_eq!(e[0].data, "{\"x\":\"你好\"}\n{\"y\":1}");
    }
    #[test]
    fn done_is_terminal_and_limit_is_enforced() {
        let mut d = SseDecoder::new(32).unwrap();
        assert_eq!(d.push(b"data: [DONE]\n\n").unwrap()[0].data, "[DONE]");
        assert!(d.push(b"data: x\n\n").is_err());
        let mut small = SseDecoder::new(4).unwrap();
        assert!(small.push(b"data: 12345\n").is_err());
    }

    #[test]
    fn supports_bare_cr_and_limits_each_event_not_total_buffer() {
        let mut decoder = SseDecoder::new(10).unwrap();
        let mut events = decoder.push(b"data:a\r\rdata:b\r\r").unwrap();
        events.extend(decoder.finish().unwrap());
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].data, "a");
        assert_eq!(events[1].data, "b");
    }
}
