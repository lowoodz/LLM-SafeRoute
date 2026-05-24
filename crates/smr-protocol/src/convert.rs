//! Convert OpenAI chat/completions requests to Anthropic /v1/messages format.

use serde_json::{json, Value};

pub fn openai_to_anthropic(body: &Value) -> Value {
    let messages = body.get("messages").and_then(|m| m.as_array()).cloned().unwrap_or_default();
    let mut system_parts: Vec<String> = Vec::new();
    let mut out_messages: Vec<Value> = Vec::new();

    for msg in messages {
        let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("user");
        match role {
            "system" => {
                if let Some(s) = msg.get("content").and_then(|c| c.as_str()) {
                    system_parts.push(s.to_string());
                }
            }
            "tool" => {
                let content = msg.get("content").and_then(|c| c.as_str()).unwrap_or("");
                let tool_call_id = msg.get("tool_call_id").and_then(|c| c.as_str()).unwrap_or("tool");
                out_messages.push(json!({
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": tool_call_id,
                        "content": content
                    }]
                }));
            }
            "assistant" => {
                let mut blocks: Vec<Value> = Vec::new();
                if let Some(text) = msg.get("content").and_then(|c| c.as_str()) {
                    if !text.is_empty() {
                        blocks.push(json!({"type": "text", "text": text}));
                    }
                }
                if let Some(tool_calls) = msg.get("tool_calls").and_then(|t| t.as_array()) {
                    for tc in tool_calls {
                        let id = tc.get("id").and_then(|v| v.as_str()).unwrap_or("tool");
                        let name = tc.get("function").and_then(|f| f.get("name")).and_then(|n| n.as_str()).unwrap_or("");
                        let args = tc.get("function").and_then(|f| f.get("arguments")).and_then(|a| a.as_str()).unwrap_or("{}");
                        let input: Value = serde_json::from_str(args).unwrap_or(json!(args));
                        blocks.push(json!({
                            "type": "tool_use",
                            "id": id,
                            "name": name,
                            "input": input
                        }));
                    }
                }
                if !blocks.is_empty() {
                    out_messages.push(json!({"role": "assistant", "content": blocks}));
                }
            }
            _ => {
                let content = msg.get("content").cloned().unwrap_or(json!(""));
                out_messages.push(json!({"role": role, "content": content}));
            }
        }
    }

    let max_tokens = body.get("max_tokens").and_then(|v| v.as_u64()).unwrap_or(4096);
    let mut out = json!({
        "model": body.get("model").cloned().unwrap_or(json!("")),
        "max_tokens": max_tokens,
        "messages": out_messages,
    });
    if !system_parts.is_empty() {
        out["system"] = json!(system_parts.join("\n"));
    }
    if let Some(temp) = body.get("temperature") {
        out["temperature"] = temp.clone();
    }
    if let Some(stream) = body.get("stream") {
        out["stream"] = stream.clone();
    }
    out
}

pub fn anthropic_to_openai(body: &Value) -> Value {
    let mut messages: Vec<Value> = Vec::new();
    if let Some(system) = body.get("system").and_then(|s| s.as_str()) {
        messages.push(json!({"role": "system", "content": system}));
    }
    if let Some(msgs) = body.get("messages").and_then(|m| m.as_array()) {
        for msg in msgs {
            let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("user");
            if let Some(content) = msg.get("content").and_then(|c| c.as_str()) {
                messages.push(json!({"role": role, "content": content}));
                continue;
            }
            if let Some(blocks) = msg.get("content").and_then(|c| c.as_array()) {
                if role == "assistant" {
                    let mut text = String::new();
                    let mut tool_calls = Vec::new();
                    for b in blocks {
                        match b.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                            "text" => text.push_str(b.get("text").and_then(|t| t.as_str()).unwrap_or("")),
                            "tool_use" => tool_calls.push(json!({
                                "id": b.get("id").cloned().unwrap_or(json!("tool")),
                                "type": "function",
                                "function": {
                                    "name": b.get("name").cloned().unwrap_or(json!("")),
                                    "arguments": b.get("input").cloned().unwrap_or(json!({})).to_string()
                                }
                            })),
                            _ => {}
                        }
                    }
                    let mut m = json!({"role": "assistant", "content": text});
                    if !tool_calls.is_empty() {
                        m["tool_calls"] = json!(tool_calls);
                    }
                    messages.push(m);
                } else {
                    for b in blocks {
                        if b.get("type").and_then(|t| t.as_str()) == Some("tool_result") {
                            messages.push(json!({
                                "role": "tool",
                                "tool_call_id": b.get("tool_use_id").cloned().unwrap_or(json!("tool")),
                                "content": b.get("content").cloned().unwrap_or(json!(""))
                            }));
                        }
                    }
                }
            }
        }
    }
    json!({
        "model": body.get("model").cloned().unwrap_or(json!("")),
        "messages": messages,
        "stream": body.get("stream").cloned().unwrap_or(json!(false))
    })
}

pub fn target_path(source_path: &str, to_anthropic: bool) -> String {
    if to_anthropic {
        if source_path.contains("chat/completions") {
            return source_path.replace("chat/completions", "messages");
        }
        return "/v1/messages".to_string();
    }
    if source_path.contains("messages") {
        return source_path.replace("messages", "chat/completions");
    }
    "/v1/chat/completions".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn converts_basic_openai_request() {
        let body = json!({
            "model": "claude",
            "messages": [
                {"role": "system", "content": "You are helpful"},
                {"role": "user", "content": "hi"}
            ]
        });
        let out = openai_to_anthropic(&body);
        assert_eq!(out["system"], "You are helpful");
        assert_eq!(out["messages"].as_array().unwrap().len(), 1);
    }
}
