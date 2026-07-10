use std::collections::HashMap;

/// Encode a machine-readable client error: `CODE` or `CODE|k=v,k2=v2`.
pub fn encode(code: &str, params: &[(&str, &str)]) -> String {
    if params.is_empty() {
        return code.to_string();
    }
    let tail = params
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(",");
    format!("{code}|{tail}")
}

pub fn decode(raw: &str) -> (String, HashMap<String, String>) {
    let Some((code, tail)) = raw.split_once('|') else {
        return (raw.to_string(), HashMap::new());
    };
    let mut params = HashMap::new();
    for part in tail.split(',') {
        if let Some((k, v)) = part.split_once('=') {
            params.insert(k.to_string(), v.to_string());
        }
    }
    (code.to_string(), params)
}
