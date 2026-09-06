use std::io::{self, Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

pub fn initialize(id: u64, root_uri: &str) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "initialize",
        "params": {
            "processId": null,
            "rootUri": root_uri,
            "capabilities": {},
            "initializationOptions": {"rustfmt": {"rangeFormatting": {"enable": true}}},
            "settings": {"rustfmt": {"rangeFormatting": {"enable": true}}}
        }
    })
}

pub fn initialized() -> serde_json::Value {
    serde_json::json!({"jsonrpc": "2.0", "method": "initialized", "params": {}})
}

pub fn did_open(uri: &str, language_id: &str, text: &str) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didOpen",
        "params": {"textDocument": {"uri": uri, "languageId": language_id, "version": 1, "text": text}}
    })
}

pub fn range_formatting(
    id: u64,
    uri: &str,
    start_line: u32,
    start_character: u32,
    end_line: u32,
    end_character: u32,
) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "textDocument/rangeFormatting",
        "params": {
            "textDocument": {"uri": uri},
            "range": {
                "start": {"line": start_line, "character": start_character},
                "end": {"line": end_line, "character": end_character}
            },
            "options": {"tabSize": 4, "insertSpaces": true}
        }
    })
}

pub fn line_range(text: &str, start: usize, end: usize) -> io::Result<(u32, u32, u32, u32)> {
    if start == 0 || end < start {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid line range"));
    }
    let lines: Vec<&str> = text.lines().collect();
    if end > lines.len() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "line range exceeds document"));
    }
    let start_line = (start - 1) as u32;
    let end_line = (end - 1) as u32;
    let end_character = lines[end - 1].encode_utf16().count() as u32;
    Ok((start_line, 0, end_line, end_character))
}

pub fn apply_text_edits(text: &str, edits: &[serde_json::Value]) -> io::Result<String> {
    let mut replacements = Vec::with_capacity(edits.len());
    for edit in edits {
        let range = edit
            .get("range")
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "TextEdit missing range"))?;
        let start = position_to_byte(text, &range["start"])?;
        let end = position_to_byte(text, &range["end"])?;
        if end < start {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "TextEdit range is reversed"));
        }
        let replacement = edit["newText"].as_str().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "TextEdit missing newText")
        })?;
        replacements.push((start, end, replacement.to_string()));
    }
    replacements.sort_by(|left, right| right.0.cmp(&left.0).then(right.1.cmp(&left.1)));
    for window in replacements.windows(2) {
        if window[0].0 < window[1].1 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "overlapping TextEdits"));
        }
    }
    let mut output = text.to_string();
    for (start, end, replacement) in replacements {
        output.replace_range(start..end, &replacement);
    }
    Ok(output)
}

pub struct Session {
    child: Child,
    stdin: ChildStdin,
    responses: Receiver<io::Result<serde_json::Value>>,
    next_id: u64,
    timeout: Duration,
}

impl Session {
    pub fn start(binary: &str, root_uri: &str, timeout: Duration) -> io::Result<Self> {
        let mut child = Command::new(binary)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("rust-analyzer stdin unavailable"))?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("rust-analyzer stdout unavailable"))?;
        let (sender, responses) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buffer = Vec::new();
            let mut chunk = [0_u8; 8192];
            loop {
                match stdout.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(count) => {
                        buffer.extend_from_slice(&chunk[..count]);
                        loop {
                            match decode(&mut buffer) {
                                Ok(Some(value)) => {
                                    if sender.send(Ok(value)).is_err() {
                                        return;
                                    }
                                }
                                Ok(None) => break,
                                Err(error) => {
                                    let _ = sender.send(Err(error));
                                    return;
                                }
                            }
                        }
                    }
                    Err(error) => {
                        let _ = sender.send(Err(error));
                        return;
                    }
                }
            }
            let _ = sender.send(Err(io::Error::new(io::ErrorKind::UnexpectedEof, "LSP stdout closed")));
        });
        let mut session = Self {
            child,
            stdin,
            responses,
            next_id: 1,
            timeout,
        };
        let initialize_id = session.next_id();
        session.send(&initialize(initialize_id, root_uri))?;
        let response = session.receive(initialize_id)?;
        response_error(&response, initialize_id)?;
        session.send(&initialized())?;
        Ok(session)
    }

    pub fn request(&mut self, message: &serde_json::Value) -> io::Result<serde_json::Value> {
        let id = message["id"].as_u64().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "LSP request missing numeric id")
        })?;
        self.send(message)?;
        let response = self.receive(id)?;
        response_error(&response, id)?;
        Ok(response["result"].clone())
    }

    pub fn notify(&mut self, message: &serde_json::Value) -> io::Result<()> {
        if message.get("id").is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "LSP notification must not have an id",
            ));
        }
        self.send(message)
    }

    pub fn shutdown(mut self) -> io::Result<()> {
        let id = self.next_id();
        self.send(&shutdown(id))?;
        let response = self.receive(id)?;
        response_error(&response, id)?;
        self.send(&serde_json::json!({"jsonrpc":"2.0","method":"exit"}))?;
        let deadline = std::time::Instant::now() + self.timeout;
        let status = loop {
            if let Some(status) = self.child.try_wait()? {
                break status;
            }
            if std::time::Instant::now() >= deadline {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "LSP shutdown timed out"));
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        if status.success() { Ok(()) } else { Err(io::Error::other("rust-analyzer exited unsuccessfully")) }
    }

    fn next_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn send(&mut self, message: &serde_json::Value) -> io::Result<()> {
        self.stdin.write_all(&encode(message))?;
        self.stdin.flush()
    }

    fn receive(&mut self, expected_id: u64) -> io::Result<serde_json::Value> {
        loop {
            let value = match self.responses.recv_timeout(self.timeout) {
                Ok(result) => result?,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    self.terminate();
                    return Err(io::Error::new(io::ErrorKind::TimedOut, "LSP response timed out"));
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.terminate();
                    return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "LSP reader disconnected"));
                }
            };
            if value["id"].as_u64() == Some(expected_id) {
                return Ok(value);
            }
        }
    }

    fn terminate(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            self.terminate();
        }
    }
}

fn position_to_byte(text: &str, position: &serde_json::Value) -> io::Result<usize> {
    let line = position["line"].as_u64().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "LSP position missing line")
    })? as usize;
    let character = position["character"].as_u64().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "LSP position missing character")
    })? as usize;
    let mut line_start = 0;
    for (index, current) in text.split_inclusive('\n').enumerate() {
        if index == line {
            return utf16_column_to_byte(&text[line_start..line_start + current.len()], character)
                .map(|offset| line_start + offset);
        }
        line_start += current.len();
    }
    if line == text.lines().count() && text.ends_with('\n') && character == 0 {
        return Ok(text.len());
    }
    Err(io::Error::new(io::ErrorKind::InvalidData, "LSP position line out of bounds"))
}

fn utf16_column_to_byte(line: &str, target: usize) -> io::Result<usize> {
    let mut units = 0;
    for (offset, ch) in line.char_indices() {
        if units == target {
            return Ok(offset);
        }
        units += ch.len_utf16();
        if units > target {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "LSP position splits UTF-16 character"));
        }
    }
    if units == target {
        Ok(line.len())
    } else {
        Err(io::Error::new(io::ErrorKind::InvalidData, "LSP position column out of bounds"))
    }
}

pub fn shutdown(id: u64) -> serde_json::Value {
    serde_json::json!({"jsonrpc": "2.0", "id": id, "method": "shutdown"})
}

pub fn response_error(value: &serde_json::Value, expected_id: u64) -> io::Result<()> {
    if value["jsonrpc"] != "2.0" || value["id"] != expected_id {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unexpected JSON-RPC response id or version",
        ));
    }
    if let Some(error) = value.get("error") {
        return Err(io::Error::other(error.to_string()));
    }
    if !value.as_object().is_some_and(|object| object.contains_key("result")) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "JSON-RPC response missing result",
        ));
    }
    Ok(())
}

pub fn encode(message: &serde_json::Value) -> Vec<u8> {
    let body = serde_json::to_vec(message).expect("JSON value must serialize");
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    let mut frame = header.into_bytes();
    frame.extend_from_slice(&body);
    frame
}

pub fn decode(buffer: &mut Vec<u8>) -> io::Result<Option<serde_json::Value>> {
    let Some(header_end) = buffer.windows(4).position(|window| window == b"\r\n\r\n") else {
        return Ok(None);
    };
    let header = std::str::from_utf8(&buffer[..header_end])
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let length = header
        .lines()
        .find_map(|line| line.strip_prefix("Content-Length:"))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing Content-Length"))?
        .trim()
        .parse::<usize>()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let body_start = header_end + 4;
    if buffer.len() < body_start + length {
        return Ok(None);
    }
    let body = buffer[body_start..body_start + length].to_vec();
    buffer.drain(..body_start + length);
    let value = serde_json::from_slice(&body)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    Ok(Some(value))
}

#[cfg(test)]
mod tests {
    use super::{decode, encode};

    #[test]
    fn round_trip_handles_fragmented_frame() {
        let expected = serde_json::json!({"id": 1, "method": "initialize"});
        let frame = encode(&expected);
        let mut buffer = frame[..10].to_vec();
        assert!(decode(&mut buffer).unwrap().is_none());
        buffer.extend_from_slice(&frame[10..]);
        assert_eq!(decode(&mut buffer).unwrap(), Some(expected));
        assert!(buffer.is_empty());
    }

    #[test]
    fn decode_keeps_following_frame() {
        let first = encode(&serde_json::json!({"id": 1}));
        let second = encode(&serde_json::json!({"id": 2}));
        let mut buffer = [first, second].concat();
        assert_eq!(decode(&mut buffer).unwrap(), Some(serde_json::json!({"id": 1})));
        assert_eq!(decode(&mut buffer).unwrap(), Some(serde_json::json!({"id": 2})));
    }

    #[test]
    fn decode_rejects_missing_content_length() {
        let mut buffer = b"X-Test: 1\r\n\r\n{}".to_vec();
        assert!(decode(&mut buffer).is_err());
    }

    #[test]
    fn request_builders_use_lsp_shapes() {
        let init = super::initialize(1, "file:///repo");
        assert_eq!(init["method"], "initialize");
        assert_eq!(init["params"]["rootUri"], "file:///repo");

        let open = super::did_open("file:///repo/src/lib.rs", "rust", "fn main() {}");
        assert_eq!(open["params"]["textDocument"]["version"], 1);

        let range = super::range_formatting(2, "file:///repo/src/lib.rs", 3, 0, 8, 12);
        assert_eq!(range["params"]["range"]["end"]["line"], 8);

        let shutdown = super::shutdown(3);
        assert_eq!(shutdown["method"], "shutdown");
        assert_eq!(super::initialized()["method"], "initialized");
    }

    #[test]
    fn response_validation_rejects_late_and_error_responses() {
        let ok = serde_json::json!({"jsonrpc":"2.0","id":7,"result":{}});
        assert!(super::response_error(&ok, 7).is_ok());
        assert!(super::response_error(&ok, 8).is_err());

        let error = serde_json::json!({"jsonrpc":"2.0","id":7,"error":{"code":-1}});
        assert!(super::response_error(&error, 7).is_err());
    }

    #[test]
    fn line_range_uses_zero_based_utf16_columns() {
        let text = "alpha\n中文😀\nomega\n";
        assert_eq!(super::line_range(text, 2, 2).unwrap(), (1, 0, 1, 4));
        assert!(super::line_range(text, 0, 1).is_err());
        assert!(super::line_range(text, 1, 4).is_err());
    }

    #[test]
    fn text_edits_apply_in_reverse_order_with_utf16_positions() {
        let text = "alpha\n中文😀\nomega\n";
        let edits = vec![
            serde_json::json!({"range":{"start":{"line":1,"character":0},"end":{"line":1,"character":4}},"newText":"rust"}),
            serde_json::json!({"range":{"start":{"line":2,"character":0},"end":{"line":2,"character":5}},"newText":"END"}),
        ];
        assert_eq!(super::apply_text_edits(text, &edits).unwrap(), "alpha\nrust\nEND\n");
    }

    #[test]
    fn text_edits_reject_overlap_and_split_surrogate() {
        let overlap = vec![
            serde_json::json!({"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":2}},"newText":"x"}),
            serde_json::json!({"range":{"start":{"line":0,"character":1},"end":{"line":0,"character":3}},"newText":"y"}),
        ];
        assert!(super::apply_text_edits("abcd", &overlap).is_err());
        let split = serde_json::json!({"range":{"start":{"line":0,"character":6},"end":{"line":0,"character":6}},"newText":"x"});
        assert!(super::apply_text_edits("😀", &[split]).is_err());
    }

    #[test]
    #[ignore = "requires rust-analyzer on PATH"]
    fn real_rust_analyzer_range_formatting_probe() {
        let root = std::env::current_dir().unwrap();
        let root_uri = format!("file://{}", root.display());
        let file = root.join("src/lsp.rs");
        let uri = format!("file://{}", file.display());
        let text = std::fs::read_to_string(&file).unwrap();
        let mut session = super::Session::start(
            "rust-analyzer",
            &root_uri,
            std::time::Duration::from_secs(10),
        )
        .unwrap();
        session.notify(&super::did_open(&uri, "rust", &text)).unwrap();
        let result = session
            .request(&super::range_formatting(2, &uri, 0, 0, 4, 0))
            .unwrap();
        assert!(result.is_array() || result.is_null());
        session.shutdown().unwrap();
    }
}
