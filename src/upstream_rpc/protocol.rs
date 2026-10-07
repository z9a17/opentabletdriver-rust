//! StreamJsonRpc's default JsonMessageFormatter + HeaderDelimitedMessageHandler.
//! https://microsoft.github.io/vs-streamjsonrpc/docs/proxies.html
//! https://microsoft.github.io/vs-streamjsonrpc/docs/sendrequest.html
use std::io;
use serde_json::{Value, json};

pub const MAX_BODY: usize = 256 * 1024;
pub const MAX_HEADER: usize = 4096;

#[derive(Debug)]
pub struct Error { pub code: i32, pub message: String }
impl Error {
    pub fn invalid(message: impl Into<String>) -> Self { Self { code: -32602, message: message.into() } }
    pub fn failed(message: impl Into<String>) -> Self { Self { code: -32000, message: message.into() } }
    pub fn unsupported(method: &str, reason: &str) -> Self {
        Self { code: -32004, message: format!("{method} is not supported: {reason}") }
    }
}
impl From<String> for Error { fn from(message: String) -> Self { Self::failed(message) } }

pub trait Service { fn invoke(&mut self, method: &str, params: &Value) -> Result<Value, Error>; }

pub fn response(body: &[u8], service: &mut impl Service) -> Option<Value> {
    let request: Value = match serde_json::from_slice(body) {
        Ok(value) => value,
        Err(_) => return Some(error(Value::Null, -32700, "Invalid JSON")),
    };
    let Some(object) = request.as_object() else {
        return Some(error(Value::Null, -32600, "Expected a JSON-RPC request object"));
    };
    let id = object.get("id").cloned();
    let usable_id = id.as_ref().filter(|value| value.is_null() || value.is_number()
        || value.as_str().is_some_and(|text| text.len() <= MAX_HEADER));
    if object.get("jsonrpc") != Some(&json!("2.0")) || object.get("method").and_then(Value::as_str).is_none()
        || (id.is_some() && usable_id.is_none()) {
        return Some(error(usable_id.cloned().unwrap_or(Value::Null), -32600, "Invalid JSON-RPC request"));
    }
    let params = object.get("params").unwrap_or(&Value::Null);
    let result = if params.is_null() || params.is_array() || params.is_object() {
        service.invoke(object["method"].as_str().unwrap(), params)
    } else {
        Err(Error::invalid("params must be an array or object"))
    };
    // Notifications execute but never receive a response, including failures.
    id.map(|id| match result {
        Ok(value) => json!({"jsonrpc":"2.0","id":id,"result":value}),
        Err(failure) => error(id, failure.code, &failure.message),
    })
}
fn error(id: Value, code: i32, message: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
}
pub fn event(name: &str, argument: Value) -> Value {
    // Strong proxies omit EventHandler's local sender, including EventArgs.Empty.
    json!({"jsonrpc":"2.0","method":name,"params":[argument]})
}
pub fn argument<'a>(params: &'a Value, name: &str) -> Result<&'a Value, Error> {
    match params {
        Value::Array(values) if values.len() == 1 => Ok(&values[0]),
        Value::Object(values) if values.len() == 1 => values.get(name).ok_or_else(|| Error::invalid(format!("expected parameter {name}"))),
        _ => Err(Error::invalid(format!("expected one parameter ({name})"))),
    }
}
pub fn no_arguments(params: &Value) -> Result<(), Error> {
    if params.is_null() || params.as_array().is_some_and(Vec::is_empty) || params.as_object().is_some_and(serde_json::Map::is_empty) {
        Ok(())
    } else { Err(Error::invalid("method takes no parameters")) }
}

/// Check all headers before allocating a body. Duplicate lengths, arithmetic
/// overflow and alternate encodings fail closed, even if the last value fits.
pub fn body_length(header: &[u8]) -> io::Result<usize> {
    let invalid = || io::Error::new(io::ErrorKind::InvalidData, "invalid Content-Length RPC header");
    if header.len() > MAX_HEADER || !header.ends_with(b"\r\n\r\n") { return Err(invalid()); }
    let text = std::str::from_utf8(&header[..header.len() - 4]).map_err(|_| invalid())?;
    let mut length = None;
    for line in text.split("\r\n") {
        let (name, value) = line.split_once(':').ok_or_else(invalid)?;
        if name.eq_ignore_ascii_case("Content-Length") {
            if length.is_some() || value.trim().is_empty() || !value.trim().bytes().all(|byte| byte.is_ascii_digit()) { return Err(invalid()); }
            let count: usize = value.trim().parse().map_err(|_| invalid())?;
            if !(1..=MAX_BODY).contains(&count) { return Err(invalid()); }
            length = Some(count);
        } else if name.eq_ignore_ascii_case("Content-Type") {
            for component in value.split(';').skip(1) {
                let (name, value) = component.trim().split_once('=').ok_or_else(invalid)?;
                if name.eq_ignore_ascii_case("charset") && !value.trim_matches('"').eq_ignore_ascii_case("utf-8") { return Err(invalid()); }
            }
        }
        if name.is_empty() || !name.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-') { return Err(invalid()); }
    }
    length.ok_or_else(invalid)
}
pub fn encode(value: &Value) -> io::Result<Vec<u8>> {
    let body = serde_json::to_vec(value).map_err(io::Error::other)?;
    if body.len() > MAX_BODY { return Err(io::Error::new(io::ErrorKind::InvalidData, "RPC response exceeds 256 KiB")); }
    let mut frame = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    frame.extend_from_slice(&body);
    Ok(frame)
}
pub fn encode_response(value: &Value) -> io::Result<Vec<u8>> {
    match encode(value) {
        Ok(frame) => Ok(frame),
        Err(failure) if failure.kind() == io::ErrorKind::InvalidData => {
            // Inventory/configuration results can legitimately exceed the frame
            // budget. Return a correlated explicit error instead of silently
            // disconnecting after executing the caller's method.
            encode(&error(value["id"].clone(), -32000, "RPC result exceeds 256 KiB; use the native bounded/paged API"))
        }
        Err(failure) => Err(failure),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Echo { calls: usize }
    impl Service for Echo {
        fn invoke(&mut self, _: &str, params: &Value) -> Result<Value, Error> {
            self.calls += 1; Ok(argument(params, "message")?.clone())
        }
    }
    #[test]
    fn streamjsonrpc_string_ids_params_and_notifications() {
        let mut service = Echo { calls: 0 };
        for params in [json!(["héllo"]), json!({"message":"héllo"})] {
            let body = serde_json::to_vec(&json!({"jsonrpc":"2.0","id":"token","method":"Echo","params":params})).unwrap();
            let reply = response(&body, &mut service).unwrap();
            assert_eq!(reply["id"], "token"); assert_eq!(reply["result"], "héllo");
            let frame = encode(&reply).unwrap();
            let boundary = frame.windows(4).position(|window| window == b"\r\n\r\n").unwrap() + 4;
            assert_eq!(body_length(&frame[..boundary]).unwrap(), frame.len() - boundary);
        }
        assert!(response(br#"{"jsonrpc":"2.0","method":"Echo","params":[1]}"#, &mut service).is_none());
        assert_eq!(service.calls, 3);
        assert_eq!(event("Resynchronize", json!({}))["params"], json!([{}]));
        assert_eq!(response(br#"{"jsonrpc":"2.0","id":2,"method":"Echo","params":false}"#, &mut service).unwrap()["error"]["code"], -32602);
        assert_eq!(service.calls, 3);
    }
    #[test]
    fn malicious_framing_is_rejected_before_body_allocation() {
        for header in [b"Content-Length: 0\r\n\r\n".as_slice(), b"Content-Length: 262145\r\n\r\n", b"Content-Length: 2\r\nContent-Length: 1\r\n\r\n", b"Content-Length: -1\r\n\r\n", b"Content-Length: 184467440737095516161\r\n\r\n", b"Content-Length: 2\r\nContent-Type: application/json; charset=utf-16\r\n\r\n", b"Content-Length: 2\n\n"] {
            assert!(body_length(header).is_err());
        }
        assert_eq!(body_length(b"content-length: 2\r\nContent-Type: application/vscode-jsonrpc; charset=utf-8\r\n\r\n").unwrap(), 2);
    }
    #[test]
    fn oversized_method_result_preserves_correlation_and_bounded_ids() {
        let response = json!({"jsonrpc":"2.0","id":"inventory","result":"x".repeat(MAX_BODY)});
        let frame = encode_response(&response).unwrap();
        let boundary = frame.windows(4).position(|bytes| bytes == b"\r\n\r\n").unwrap() + 4;
        let reply: Value = serde_json::from_slice(&frame[boundary..]).unwrap();
        assert_eq!(reply["id"],"inventory");
        assert_eq!(reply["error"]["code"],-32000);
        let mut service = Echo { calls:0 };
        let request = serde_json::to_vec(&json!({"jsonrpc":"2.0","id":"x".repeat(MAX_HEADER + 1),"method":"Echo","params":[1]})).unwrap();
        assert_eq!(super::response(&request,&mut service).unwrap()["error"]["code"],-32600);
        assert_eq!(service.calls,0);
    }
}
