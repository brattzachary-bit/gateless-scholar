//! Robust extraction of JSON from model replies (code fences, prose around it, `<think>`
//! blocks, trailing commas, smart quotes as delimiters).

use serde_json::Value;

use crate::providers::stream::strip_think;

/// Find and parse the JSON object (or array) in a model reply.
pub fn extract_json(reply: &str) -> Option<Value> {
    let text = strip_think(reply);
    let text = strip_fences(&text);
    for candidate in candidates(&text) {
        if let Ok(v) = serde_json::from_str::<Value>(&candidate) {
            return Some(v);
        }
        let fixed = fix_common(&candidate);
        if let Ok(v) = serde_json::from_str::<Value>(&fixed) {
            return Some(v);
        }
    }
    None
}

fn strip_fences(s: &str) -> String {
    let t = s.trim();
    if let Some(start) = t.find("```") {
        let after = &t[start + 3..];
        let after = after.strip_prefix("json").or_else(|| after.strip_prefix("JSON")).unwrap_or(after);
        if let Some(end) = after.find("```") {
            return after[..end].trim().to_string();
        }
        return after.trim().to_string();
    }
    t.to_string()
}

fn candidates(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let obj = s.find('{').zip(s.rfind('}')).filter(|(a, b)| a < b).map(|(a, b)| s[a..=b].to_string());
    let arr = s.find('[').zip(s.rfind(']')).filter(|(a, b)| a < b).map(|(a, b)| s[a..=b].to_string());
    // Prefer whichever opens first.
    match (s.find('{'), s.find('[')) {
        (Some(a), Some(b)) if b < a => {
            out.extend(arr);
            out.extend(obj);
        }
        _ => {
            out.extend(obj);
            out.extend(arr);
        }
    }
    // A truncated reply: try closing open brackets.
    if let Some(a) = s.find('{') {
        out.push(close_open(&s[a..]));
    }
    out
}

fn fix_common(s: &str) -> String {
    let re = regex::Regex::new(r",\s*([}\]])").expect("regex");
    let s = re.replace_all(s, "$1").to_string();
    s.replace(['“', '”'], "\"")
}

/// Close unbalanced brackets/strings of a truncated JSON text (best effort).
fn close_open(s: &str) -> String {
    let mut stack = Vec::new();
    let mut in_str = false;
    let mut esc = false;
    for c in s.chars() {
        if in_str {
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '{' => stack.push('}'),
            '[' => stack.push(']'),
            '}' | ']' => {
                stack.pop();
            }
            _ => {}
        }
    }
    let mut out = s.trim_end().trim_end_matches(',').to_string();
    if in_str {
        out.push('"');
    }
    while let Some(c) = stack.pop() {
        out.push(c);
    }
    out
}

/// Read a string field, tolerating numbers and nulls.
pub fn str_field(v: &Value, key: &str) -> Option<String> {
    match v.get(key)? {
        Value::String(s) => Some(s.trim().to_string()).filter(|s| !s.is_empty()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// Read a list of strings (or a single string) field.
pub fn str_list(v: &Value, key: &str) -> Vec<String> {
    match v.get(key) {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|x| match x {
                Value::String(s) => Some(s.trim().to_string()),
                Value::Object(_) => x.get("text").or(x.get("query")).and_then(|t| t.as_str()).map(|s| s.trim().to_string()),
                _ => None,
            })
            .filter(|s| !s.is_empty())
            .collect(),
        Some(Value::String(s)) if !s.trim().is_empty() => vec![s.trim().to_string()],
        _ => vec![],
    }
}

/// Read a list of integers (numbers, numeric strings, or "[3]" strings).
pub fn int_list(v: &Value, key: &str) -> Vec<i64> {
    let as_int = |x: &Value| -> Option<i64> {
        match x {
            Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
            Value::String(s) => s.trim().trim_matches(['[', ']', '#']).trim().parse().ok(),
            _ => None,
        }
    };
    match v.get(key) {
        Some(Value::Array(a)) => a.iter().filter_map(as_int).collect(),
        Some(x) => as_int(x).into_iter().collect(),
        None => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_from_fences_and_prose() {
        let v = extract_json("Sure! Here it is:\n```json\n{\"a\": 1, \"b\": [1,2,],}\n```\nHope this helps").unwrap();
        assert_eq!(v["a"], 1);
        assert_eq!(v["b"][1], 2);
        let v = extract_json("<think>hmm {not json}</think>{\"title\": \"X\"}").unwrap();
        assert_eq!(v["title"], "X");
        let v = extract_json("[\"q1\", \"q2\"]").unwrap();
        assert!(v.is_array());
    }

    #[test]
    fn repairs_truncated_reply() {
        let v = extract_json("{\"queries\": [\"a\", \"b\"").unwrap();
        assert_eq!(str_list(&v, "queries"), vec!["a", "b"]);
    }

    #[test]
    fn lenient_fields() {
        let v: Value = serde_json::json!({"n": ["[3]", 4, "x"], "s": 5, "l": "one"});
        assert_eq!(int_list(&v, "n"), vec![3, 4]);
        assert_eq!(str_field(&v, "s").as_deref(), Some("5"));
        assert_eq!(str_list(&v, "l"), vec!["one"]);
        assert!(extract_json("no json here").is_none());
    }
}
