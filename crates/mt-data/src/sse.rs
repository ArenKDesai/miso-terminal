//! Server-sent events (`text/event-stream`): how streaming APIs (the Claude
//! Messages API) send an answer while it is being written.
//!
//! [`SseParser`] takes the body in whatever pieces the network delivers and
//! returns whole events. It follows the WHATWG rules the APIs use: lines end
//! with `\n`, `\r\n` or `\r`; a blank line ends an event; `data:` lines are
//! joined with `\n`; lines starting with `:` are comments (keep-alives);
//! one space after the colon is dropped.

/// One event: its `event:` name (empty when none was given) and its data.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SseEvent {
    pub event: String,
    pub data: String,
}

/// Turns body chunks into events. Feed it with [`Self::push`]; call
/// [`Self::finish`] when the body ends.
#[derive(Debug, Default)]
pub struct SseParser {
    /// Bytes of a line not yet ended.
    partial: Vec<u8>,
    /// The previous chunk ended with `\r`: a `\n` starting the next is part of it.
    after_cr: bool,
    event: String,
    data: Vec<String>,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a piece of the body; returns the events it completed.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        let mut out = Vec::new();
        let mut bytes = chunk;
        if self.after_cr && bytes.first() == Some(&b'\n') {
            bytes = &bytes[1..];
        }
        self.after_cr = false;
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'\n' | b'\r' => {
                    let line = std::mem::take(&mut self.partial);
                    self.line(&line, &mut out);
                    if bytes[i] == b'\r' {
                        if i + 1 == bytes.len() {
                            self.after_cr = true;
                        } else if bytes[i + 1] == b'\n' {
                            i += 1;
                        }
                    }
                }
                b => self.partial.push(b),
            }
            i += 1;
        }
        out
    }

    /// The body ended: an event without its closing blank line is dropped,
    /// as the standard says (a cut-off stream is the caller's error to report).
    pub fn finish(&mut self) -> Vec<SseEvent> {
        *self = Self::default();
        Vec::new()
    }

    fn line(&mut self, line: &[u8], out: &mut Vec<SseEvent>) {
        if line.is_empty() {
            if !self.data.is_empty() {
                out.push(SseEvent {
                    event: std::mem::take(&mut self.event),
                    data: std::mem::take(&mut self.data).join("\n"),
                });
            }
            self.event.clear();
            return;
        }
        let line = String::from_utf8_lossy(line);
        if line.starts_with(':') {
            return;
        }
        let (field, value) = match line.split_once(':') {
            Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
            None => (line.as_ref(), ""),
        };
        match field {
            "event" => self.event = value.to_owned(),
            "data" => self.data.push(value.to_owned()),
            // `id` and `retry` matter only to browsers that reconnect.
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(chunks: &[&[u8]]) -> Vec<SseEvent> {
        let mut p = SseParser::new();
        chunks.iter().flat_map(|c| p.push(c)).collect()
    }

    fn ev(event: &str, data: &str) -> SseEvent {
        SseEvent {
            event: event.into(),
            data: data.into(),
        }
    }

    #[test]
    fn events_in_one_piece() {
        let body =
            b"event: message_start\ndata: {\"a\":1}\n\n: keep-alive\n\nevent: ping\ndata: {}\n\n";
        assert_eq!(
            all(&[body]),
            vec![ev("message_start", "{\"a\":1}"), ev("ping", "{}")]
        );
    }

    #[test]
    fn events_split_anywhere() {
        let body: &[u8] = b"event: a\r\ndata: one\r\ndata:two\r\n\r\nevent: b\rdata: x\r\r";
        let whole = all(&[body]);
        assert_eq!(whole, vec![ev("a", "one\ntwo"), ev("b", "x")]);
        for cut in 0..body.len() {
            assert_eq!(all(&[&body[..cut], &body[cut..]]), whole, "cut at {cut}");
        }
        let bytes: Vec<&[u8]> = body.chunks(1).collect();
        assert_eq!(all(&bytes), whole);
    }

    #[test]
    fn unfinished_events_and_utf8_split_across_chunks() {
        let body = "data: ½ MWh\n\ndata: cut".as_bytes();
        let mut p = SseParser::new();
        let mut got = p.push(&body[..7]);
        got.extend(p.push(&body[7..]));
        assert_eq!(got, vec![ev("", "½ MWh")]);
        assert!(p.finish().is_empty());
    }
}
