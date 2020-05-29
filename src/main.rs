use reqwest::Client;
use serde::Deserialize;
use serde_json::{Value as JsonValue, Value};
use std::{collections::HashMap, convert::Infallible, env, fmt, process, sync::Arc};
use tokio::fs;
use warp::{Filter, Rejection, Reply};

type JsonObject = serde_json::Map<String, JsonValue>;
type JsonArray = Vec<JsonValue>;

trait TryExt<T> {
    fn unwrap_or_exit(self, message: &str, code: i32) -> T;
    fn or_reject(self) -> Result<T, Rejection>;
    fn or_log_and_reject(self, message: &str) -> Result<T, Rejection>;
}
impl<T, E: std::error::Error> TryExt<T> for Result<T, E> {
    fn unwrap_or_exit(self, message: &str, code: i32) -> T {
        self.unwrap_or_else(|e| {
            eprintln!("{}: {}", message, e);
            process::exit(code)
        })
    }
    fn or_reject(self) -> Result<T, Rejection> {
        match self {
            Ok(v) => Ok(v),
            Err(e) => {
                eprintln!("{}", e);
                Err(warp::reject())
            }
        }
    }
    fn or_log_and_reject(self, message: &str) -> Result<T, Rejection> {
        match self {
            Ok(v) => Ok(v),
            Err(e) => {
                eprintln!("{}: {}", message, e);
                Err(warp::reject())
            }
        }
    }
}
impl<T> TryExt<T> for Option<T> {
    fn unwrap_or_exit(self, message: &str, code: i32) -> T {
        self.unwrap_or_else(|| {
            eprintln!("{}", message);
            process::exit(code)
        })
    }
    fn or_reject(self) -> Result<T, Rejection> {
        match self {
            Some(v) => Ok(v),
            None => Err(warp::reject()),
        }
    }
    fn or_log_and_reject(self, message: &str) -> Result<T, Rejection> {
        match self {
            Some(v) => Ok(v),
            None => {
                eprintln!("{}", message);
                Err(warp::reject())
            }
        }
    }
}

#[derive(Copy, Clone)]
enum OrInsertJsonValue {
    Object,
    Array,
    Null,
}
impl OrInsertJsonValue {
    fn concrete(self) -> JsonValue {
        match self {
            OrInsertJsonValue::Object => JsonValue::Object(JsonObject::new()),
            OrInsertJsonValue::Array => JsonValue::Array(JsonArray::new()),
            OrInsertJsonValue::Null => JsonValue::Null,
        }
    }
}
trait JsonExt<KI> {
    fn get_or_insert_mut(&mut self, ki: KI, insert: OrInsertJsonValue) -> &mut JsonValue;
}
impl JsonExt<&str> for JsonObject {
    fn get_or_insert_mut(&mut self, key: &str, insert: OrInsertJsonValue) -> &mut Value {
        if !self.contains_key(key) {
            self.insert(key.to_owned(), insert.concrete());
        }
        self.get_mut(key).unwrap()
    }
}
impl JsonExt<usize> for JsonArray {
    fn get_or_insert_mut(&mut self, index: usize, insert: OrInsertJsonValue) -> &mut Value {
        if self.len() < index {
            for _ in self.len()..index {
                self.push(JsonValue::Null);
            }
            self.push(insert.concrete());
        }
        self.get_mut(index).unwrap()
    }
}

#[derive(Debug)]
struct StrError(&'static str);
impl fmt::Display for StrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
impl std::error::Error for StrError {}

fn inject<T: Send + Sync>(
    arc: Arc<T>,
) -> impl Filter<Extract = (Arc<T>,), Error = Infallible> + Clone {
    warp::any().map(move || arc.clone())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Config {
    port: u16,
    user_agent: Option<String>,
    webhooks: HashMap<String, Webhook>,
    #[serde(default)]
    debug: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Webhook {
    forward_url: String,
    #[serde(default)]
    forward_method: Method,
    fields: Vec<Field>,
    reply: Option<JsonObject>,
}

#[derive(Deserialize, Copy, Clone)]
enum Method {
    #[serde(rename = "POST")]
    Post,
    #[serde(rename = "PUT")]
    Put,
    #[serde(rename = "PATCH")]
    Patch,
}
impl Default for Method {
    fn default() -> Self {
        Self::Post
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Field {
    from: JsonPath,
    to: JsonPath,
    #[serde(default)]
    optional: bool,
}

type JsonPath = Vec<JsonPathSegment>;

#[derive(Deserialize)]
#[serde(untagged)]
enum JsonPathSegment {
    Key(String),
    Index(usize),
}

#[tokio::main]
async fn main() {
    let config_path = match env::args().nth(2) {
        Some(s) => s,
        None => "forwardhook.json".to_owned(),
    };
    let config_contents = fs::read_to_string(&config_path)
        .await
        .unwrap_or_exit("Can't read config file", 74);
    let config: Config =
        serde_json::from_str(&config_contents).unwrap_or_exit("Invalid config file", 66);
    let port = config.port;

    let client = Client::builder()
        .user_agent(
            config
                .user_agent
                .as_ref()
                .map(|s| s.as_ref())
                .unwrap_or(concat!("forwardhook/", env!("CARGO_PKG_VERSION"))),
        )
        .build()
        .unwrap_or_exit("Can't create web client", 65);

    let filter = warp::path!(String)
        .and(warp::body::json())
        .and(inject(Arc::new(config)))
        .and(inject(Arc::new(client)))
        .and_then(handler);

    println!("Listening on port {}", port);
    warp::serve(filter).run(([127, 0, 0, 1], port)).await;
}

async fn handler(
    id: String,
    body: JsonObject,
    config: Arc<Config>,
    client: Arc<Client>,
) -> Result<impl Reply, Rejection> {
    let debug = config.debug;
    let config = match config.webhooks.get(&id) {
        Some(c) => c,
        None => return Err(warp::reject::not_found()),
    };
    let mut forwarded = JsonObject::new();

    for field in &config.fields {
        let from = match from(field, &body, &id) {
            Ok(f) => f,
            Err(e) => {
                if field.optional {
                    continue;
                } else {
                    return Err(e);
                }
            }
        };

        macro_rules! match_peek {
            ($iter:expr) => {
                match $iter.peek() {
                    Some(JsonPathSegment::Key(_)) => OrInsertJsonValue::Object,
                    Some(JsonPathSegment::Index(_)) => OrInsertJsonValue::Array,
                    _ => OrInsertJsonValue::Null,
                }
            };
        }

        let mut to_segments = field.to.iter().peekable();
        let mut to = match to_segments.next() {
            Some(JsonPathSegment::Key(k)) => {
                forwarded.get_or_insert_mut(k, match_peek!(to_segments))
            }
            _ => return Err(warp::reject()),
        };
        while let Some(segment) = to_segments.next() {
            to = match segment {
                JsonPathSegment::Key(k) => {
                    let obj = to.as_object_mut().or_reject()?;
                    obj.get_or_insert_mut(k, match_peek!(to_segments))
                }
                JsonPathSegment::Index(i) => {
                    let ary = to.as_array_mut().or_reject()?;
                    ary.get_or_insert_mut(*i, match_peek!(to_segments))
                }
            };
        }

        *to = from.clone();
    }

    if !debug {
        match config.forward_method {
            Method::Post => client.post(&config.forward_url),
            Method::Put => client.put(&config.forward_url),
            Method::Patch => client.patch(&config.forward_url),
        }
        .json(&forwarded)
        .send()
        .await
        .or_reject()?;
        match &config.reply {
            Some(o) => Ok(warp::reply::json(o)),
            None => Ok(warp::reply::json(&JsonObject::new())),
        }
    } else {
        Ok(warp::reply::json(&forwarded))
    }
}

fn from<'a, 'b, 'c>(
    field: &'b Field,
    body: &'a JsonObject,
    id: &'c str,
) -> Result<&'a Value, Rejection> {
    let mut from_segments = field.from.iter();
    let mut from = match from_segments.next() {
        Some(JsonPathSegment::Key(k)) => body
            .get(k)
            .or_log_and_reject(&format!("Missing key `{}` in `{}`", k, id))?,
        _ => return Err(warp::reject()),
    };
    for segment in from_segments {
        from = match segment {
            JsonPathSegment::Key(k) => from
                .as_object()
                .or_log_and_reject(&format!("Expected object in `{}`", id))?
                .get(k)
                .or_log_and_reject(&format!("Missing key `{}` in `{}`", k, id))?,
            JsonPathSegment::Index(i) => from
                .as_array()
                .or_log_and_reject(&format!("Expected array in `{}`", id))?
                .get(*i)
                .or_log_and_reject(&format!("Missing index `{}` in `{}`", i, id))?,
        };
    }
    Ok(from)
}
