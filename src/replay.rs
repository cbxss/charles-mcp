use std::collections::HashMap;
use std::time::Instant;

use reqwest::Method;
use reqwest::redirect::Policy;

use crate::config::Config;
use crate::error::CharlesError;
use crate::session::{Body, RawBody, Transaction, body, charset_from_content_type};

const STRIP_HEADERS: &[&str] = &[
    "host",
    "content-length",
    "transfer-encoding",
    "connection",
    "proxy-connection",
    "keep-alive",
];

pub struct ReplayOptions {
    pub query_overrides: HashMap<String, Option<String>>,
    pub header_overrides: HashMap<String, Option<String>>,
    pub json_overrides: Option<serde_json::Value>,
    pub body_text: Option<String>,
    pub use_proxy: bool,
    pub follow_redirects: bool,
    pub max_body_bytes: usize,
}

pub struct ReplayResult {
    pub method: String,
    pub url: String,
    pub status: u16,
    pub response_headers: Vec<(String, String)>,
    pub body: Body,
    pub baseline_status: Option<u16>,
    pub elapsed_ms: u128,
    pub auth_present: bool,
    pub via_proxy: bool,
}

pub async fn replay(
    cfg: &Config,
    t: &Transaction,
    opts: &ReplayOptions,
) -> Result<ReplayResult, CharlesError> {
    let method = Method::from_bytes(t.method.to_uppercase().as_bytes())
        .map_err(|_| CharlesError::InvalidArg(format!("unsupported HTTP method '{}'", t.method)))?;

    let url = build_url(&t.url, &opts.query_overrides)?;

    let mut headers: Vec<(String, String)> = t
        .request
        .headers
        .iter()
        .filter(|(k, _)| !STRIP_HEADERS.iter().any(|s| k.eq_ignore_ascii_case(s)))
        .cloned()
        .collect();
    for (name, value) in &opts.header_overrides {
        remove_header(&mut headers, name);
        if let Some(v) = value {
            headers.push((name.clone(), v.clone()));
        }
    }

    let built = build_body(t, opts)?;
    if let Some(ct) = built.content_type {
        set_header(&mut headers, "content-type", &ct);
    }
    if built.drop_encoding {
        remove_header(&mut headers, "content-encoding");
    }
    let content = built.bytes;

    let auth_present = headers
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case("authorization") || k.eq_ignore_ascii_case("cookie"));

    let redirect = if opts.follow_redirects {
        Policy::limited(10)
    } else {
        Policy::none()
    };
    let mut builder = reqwest::Client::builder()
        .timeout(cfg.timeout())
        .redirect(redirect);
    if opts.use_proxy {
        builder = builder.proxy(reqwest::Proxy::all(cfg.proxy_url())?);
    } else {
        builder = builder.no_proxy();
    }
    let client = builder.build()?;

    let mut rb = client.request(method, &url);
    for (k, v) in &headers {
        rb = rb.header(k, v);
    }
    if let Some(bytes) = content {
        rb = rb.body(bytes);
    }

    let start = Instant::now();
    let resp = rb.send().await?;
    let elapsed_ms = start.elapsed().as_millis();
    let status = resp.status().as_u16();
    let response_headers: Vec<(String, String)> = resp
        .headers()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();
    let resp_bytes = resp.bytes().await?.to_vec();

    let raw = response_raw_body(&response_headers, resp_bytes);
    let decoded = body::decode(&raw, opts.max_body_bytes);

    Ok(ReplayResult {
        method: t.method.clone(),
        url,
        status,
        response_headers,
        body: decoded,
        baseline_status: t.status,
        elapsed_ms,
        auth_present,
        via_proxy: opts.use_proxy,
    })
}

fn build_url(
    url: &str,
    overrides: &HashMap<String, Option<String>>,
) -> Result<String, CharlesError> {
    if overrides.is_empty() {
        return Ok(url.to_string());
    }
    let mut u = url::Url::parse(url)
        .map_err(|e| CharlesError::InvalidArg(format!("bad url '{url}': {e}")))?;
    let mut pairs: Vec<(String, String)> = u
        .query_pairs()
        .filter(|(k, _)| !overrides.contains_key(k.as_ref()))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    for (k, v) in overrides {
        if let Some(val) = v {
            pairs.push((k.clone(), val.clone()));
        }
    }
    if pairs.is_empty() {
        u.set_query(None);
    } else {
        u.query_pairs_mut().clear().extend_pairs(&pairs);
    }
    Ok(u.to_string())
}

struct BuiltBody {
    bytes: Option<Vec<u8>>,
    content_type: Option<String>,
    drop_encoding: bool,
}

fn build_body(t: &Transaction, opts: &ReplayOptions) -> Result<BuiltBody, CharlesError> {
    if let Some(text) = &opts.body_text {
        return Ok(BuiltBody {
            bytes: Some(text.clone().into_bytes()),
            content_type: None,
            drop_encoding: true,
        });
    }

    if let Some(over) = &opts.json_overrides {
        let mut obj = match decoded_request_text(t) {
            Some(s) if !s.trim().is_empty() => serde_json::from_str::<serde_json::Value>(&s)
                .map_err(|e| {
                    CharlesError::InvalidArg(format!(
                        "json_overrides requires the original request body to be JSON, but it did \
                         not parse: {e}"
                    ))
                })?,
            _ => serde_json::Value::Object(serde_json::Map::new()),
        };
        if !obj.is_object() {
            return Err(CharlesError::InvalidArg(
                "json_overrides requires a JSON object request body".into(),
            ));
        }
        let map = obj.as_object_mut().expect("checked is_object");
        if let Some(over_map) = over.as_object() {
            for (k, v) in over_map {
                if v.is_null() {
                    map.remove(k);
                } else {
                    map.insert(k.clone(), v.clone());
                }
            }
        } else {
            return Err(CharlesError::InvalidArg(
                "json_overrides must itself be a JSON object of keys to merge".into(),
            ));
        }
        let bytes = serde_json::to_vec(&obj)?;
        return Ok(BuiltBody {
            bytes: Some(bytes),
            content_type: Some("application/json".into()),
            drop_encoding: true,
        });
    }

    if t.request.raw.captured && !t.request.raw.bytes.is_empty() {
        return Ok(BuiltBody {
            bytes: Some(t.request.raw.bytes.clone()),
            content_type: None,
            drop_encoding: false,
        });
    }
    Ok(BuiltBody {
        bytes: None,
        content_type: None,
        drop_encoding: false,
    })
}

fn decoded_request_text(t: &Transaction) -> Option<String> {
    match body::decode(&t.request.raw, 1 << 20) {
        Body::Text { text, .. } => Some(text),
        _ => None,
    }
}

fn response_raw_body(headers: &[(String, String)], bytes: Vec<u8>) -> RawBody {
    let ct = header_value(headers, "content-type");
    RawBody {
        bytes,
        content_encoding: header_value(headers, "content-encoding"),
        declared_charset: charset_from_content_type(ct.as_deref()),
        content_type: ct,
        grpc_encoding: header_value(headers, "grpc-encoding"),
        was_base64_wrapped: false,
        captured: true,
    }
}

fn header_value(headers: &[(String, String)], name: &str) -> Option<String> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.clone())
}

fn remove_header(headers: &mut Vec<(String, String)>, name: &str) {
    headers.retain(|(k, _)| !k.eq_ignore_ascii_case(name));
}

fn set_header(headers: &mut Vec<(String, String)>, name: &str, value: &str) {
    remove_header(headers, name);
    headers.push((name.to_string(), value.to_string()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_overrides_replace_and_remove() {
        let mut over = HashMap::new();
        over.insert("a".to_string(), Some("2".to_string()));
        over.insert("b".to_string(), None);
        over.insert("c".to_string(), Some("9".to_string()));
        let url = build_url("https://x.test/p?a=1&b=keepme&d=4", &over).unwrap();
        assert!(url.contains("a=2"), "{url}");
        assert!(!url.contains("b="), "b should be removed: {url}");
        assert!(url.contains("c=9"), "{url}");
        assert!(url.contains("d=4"), "untouched param preserved: {url}");
    }

    #[test]
    fn no_query_overrides_keeps_url_verbatim() {
        let url = build_url("https://x.test/p?z=1", &HashMap::new()).unwrap();
        assert_eq!(url, "https://x.test/p?z=1");
    }

    #[test]
    fn json_overrides_merge_into_object() {
        let mut t = Transaction::default();
        t.request.raw = RawBody {
            bytes: br#"{"keep":1,"drop":2}"#.to_vec(),
            content_type: Some("application/json".into()),
            captured: true,
            ..Default::default()
        };
        let mut over_map = serde_json::Map::new();
        over_map.insert("drop".into(), serde_json::Value::Null);
        over_map.insert("add".into(), serde_json::json!("x"));
        let opts = ReplayOptions {
            query_overrides: HashMap::new(),
            header_overrides: HashMap::new(),
            json_overrides: Some(serde_json::Value::Object(over_map)),
            body_text: None,
            use_proxy: false,
            follow_redirects: false,
            max_body_bytes: 4096,
        };
        let built = build_body(&t, &opts).unwrap();
        assert_eq!(built.content_type.as_deref(), Some("application/json"));
        assert!(built.drop_encoding);
        let v: serde_json::Value = serde_json::from_slice(&built.bytes.unwrap()).unwrap();
        assert_eq!(v["keep"], 1);
        assert_eq!(v["add"], "x");
        assert!(v.get("drop").is_none(), "drop should be removed");
    }

    fn opts_with(json: Option<serde_json::Value>, body_text: Option<String>) -> ReplayOptions {
        ReplayOptions {
            query_overrides: HashMap::new(),
            header_overrides: HashMap::new(),
            json_overrides: json,
            body_text,
            use_proxy: false,
            follow_redirects: false,
            max_body_bytes: 4096,
        }
    }

    #[test]
    fn json_overrides_with_no_original_body_start_from_empty_object() {
        let t = Transaction::default();
        let mut over = serde_json::Map::new();
        over.insert("a".into(), serde_json::json!(1));
        let built =
            build_body(&t, &opts_with(Some(serde_json::Value::Object(over)), None)).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&built.bytes.unwrap()).unwrap();
        assert_eq!(v["a"], 1);
    }

    #[test]
    fn json_overrides_on_a_non_object_body_is_a_clear_error() {
        let mut t = Transaction::default();
        t.request.raw = RawBody {
            bytes: b"[1,2,3]".to_vec(),
            content_type: Some("application/json".into()),
            captured: true,
            ..Default::default()
        };
        let mut over = serde_json::Map::new();
        over.insert("a".into(), serde_json::json!(1));
        let err = build_body(&t, &opts_with(Some(serde_json::Value::Object(over)), None));
        assert!(err.is_err(), "merging into a non-object body must error");
    }

    #[test]
    fn body_text_override_replaces_verbatim_and_drops_encoding() {
        let mut t = Transaction::default();
        t.request.raw = RawBody {
            bytes: b"original".to_vec(),
            content_encoding: Some("gzip".into()),
            captured: true,
            ..Default::default()
        };
        let built = build_body(&t, &opts_with(None, Some("brand new".into()))).unwrap();
        assert_eq!(built.bytes.as_deref(), Some(b"brand new".as_slice()));
        assert!(
            built.drop_encoding,
            "plaintext replacement drops content-encoding"
        );
    }

    #[test]
    fn query_override_adds_to_a_url_that_had_no_query() {
        let mut over = HashMap::new();
        over.insert("k".to_string(), Some("v".to_string()));
        let url = build_url("https://x.test/path", &over).unwrap();
        assert_eq!(url, "https://x.test/path?k=v");
    }

    #[test]
    fn removing_the_only_query_param_clears_the_query_string() {
        let mut over = HashMap::new();
        over.insert("only".to_string(), None);
        let url = build_url("https://x.test/p?only=1", &over).unwrap();
        assert_eq!(url, "https://x.test/p");
    }
}
