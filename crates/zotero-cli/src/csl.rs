use serde_json::{Map, Value};

const CSL_TYPE_MAP: &[(&str, &str)] = &[
    ("article-journal", "journalArticle"),
    ("article-magazine", "magazineArticle"),
    ("article-newspaper", "newspaperArticle"),
    ("article", "journalArticle"),
    ("book", "book"),
    ("chapter", "bookSection"),
    ("paper-conference", "conferencePaper"),
    ("thesis", "thesis"),
    ("report", "report"),
    ("webpage", "webpage"),
    ("post-weblog", "blogPost"),
    ("post", "forumPost"),
    ("manuscript", "manuscript"),
    ("dataset", "document"),
    ("software", "computerProgram"),
    ("patent", "patent"),
    ("bill", "bill"),
    ("map", "map"),
    ("motion_picture", "film"),
    ("song", "audioRecording"),
    ("speech", "presentation"),
    ("entry-encyclopedia", "encyclopediaArticle"),
    ("entry-dictionary", "dictionaryEntry"),
];

pub fn is_truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(value)) => *value,
        Some(Value::Number(number)) => number.as_f64() != Some(0.0),
        Some(Value::String(value)) => !value.is_empty(),
        Some(Value::Array(values)) => !values.is_empty(),
        Some(Value::Object(values)) => !values.is_empty(),
    }
}

fn first_truthy<'a>(first: Option<&'a Value>, second: Option<&'a Value>) -> Option<&'a Value> {
    first
        .filter(|value| is_truthy(Some(value)))
        .or_else(|| second.filter(|value| is_truthy(Some(value))))
}

pub fn looks_like_csl_item(obj: &Value) -> bool {
    let Some(obj) = obj.as_object() else {
        return false;
    };
    if is_truthy(obj.get("itemType")) && (obj.contains_key("creators") || obj.contains_key("title"))
    {
        return false;
    }
    let has_identifier =
        is_truthy(obj.get("type")) || is_truthy(obj.get("DOI")) || is_truthy(obj.get("title"));
    let has_csl_signal = obj.contains_key("author")
        || obj.contains_key("issued")
        || obj.contains_key("container-title")
        || obj.contains_key("id")
        || obj.contains_key("DOI");
    has_identifier && has_csl_signal
}

pub fn issued_to_date(issued: Option<&Value>) -> String {
    let Some(obj) = issued.and_then(Value::as_object) else {
        return String::new();
    };
    let Some(parts) = first_truthy(obj.get("date-parts"), obj.get("raw")) else {
        return String::new();
    };
    if let Some(raw) = parts.as_str() {
        return raw.to_string();
    }
    let Some(first) = parts
        .as_array()
        .and_then(|values| values.first())
        .and_then(Value::as_array)
    else {
        return String::new();
    };
    first
        .iter()
        .filter(|value| !value.is_null())
        .map(value_to_python_string)
        .collect::<Vec<_>>()
        .join("-")
}

pub(crate) fn value_to_python_string(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        Value::Bool(value) => {
            if *value {
                "True".to_string()
            } else {
                "False".to_string()
            }
        }
        Value::Null => "None".to_string(),
        other => other.to_string(),
    }
}

fn csl_type(value: Option<&Value>) -> &'static str {
    let key = value
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_lowercase();
    CSL_TYPE_MAP
        .iter()
        .find_map(|(from, to)| (*from == key).then_some(*to))
        .unwrap_or("journalArticle")
}

fn push_string_field(out: &mut Map<String, Value>, key: &str, value: Option<&Value>) {
    if is_truthy(value) {
        out.insert(
            key.to_string(),
            Value::String(value_to_python_string(value.unwrap())),
        );
    }
}

fn authors_to_creators(authors: Option<&Value>) -> Vec<Value> {
    let Some(authors) = authors.and_then(Value::as_array) else {
        return Vec::new();
    };
    authors
        .iter()
        .filter_map(|author| {
            let author = author.as_object()?;
            if is_truthy(author.get("literal")) {
                let mut creator = Map::new();
                creator.insert(
                    "creatorType".to_string(),
                    Value::String("author".to_string()),
                );
                creator.insert(
                    "name".to_string(),
                    Value::String(value_to_python_string(author.get("literal").unwrap())),
                );
                return Some(Value::Object(creator));
            }
            let mut creator = Map::new();
            creator.insert(
                "creatorType".to_string(),
                Value::String("author".to_string()),
            );
            creator.insert(
                "firstName".to_string(),
                Value::String(
                    author
                        .get("given")
                        .filter(|value| is_truthy(Some(value)))
                        .map(value_to_python_string)
                        .unwrap_or_default(),
                ),
            );
            creator.insert(
                "lastName".to_string(),
                Value::String(
                    author
                        .get("family")
                        .filter(|value| is_truthy(Some(value)))
                        .map(value_to_python_string)
                        .unwrap_or_default(),
                ),
            );
            Some(Value::Object(creator))
        })
        .collect()
}

fn identifier_list(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        return text.to_string();
    }
    value
        .as_array()
        .map(|values| {
            values
                .iter()
                .map(value_to_python_string)
                .collect::<Vec<_>>()
                .join(",")
        })
        .unwrap_or_else(|| value_to_python_string(value))
}

pub fn csl_item_to_connector(item: &Map<String, Value>, index: usize) -> Map<String, Value> {
    let mut out = Map::new();
    out.insert(
        "itemType".to_string(),
        Value::String(csl_type(item.get("type")).to_string()),
    );
    let title = item
        .get("title")
        .filter(|value| is_truthy(Some(value)))
        .or_else(|| {
            item.get("container-title")
                .filter(|value| is_truthy(Some(value)))
        })
        .map(value_to_python_string)
        .unwrap_or_else(|| "Untitled".to_string());
    out.insert("title".to_string(), Value::String(title));
    out.insert(
        "id".to_string(),
        Value::String(format!("cli-anything-csl-{index}")),
    );

    push_string_field(
        &mut out,
        "DOI",
        first_truthy(item.get("DOI"), item.get("doi")),
    );
    push_string_field(
        &mut out,
        "url",
        first_truthy(item.get("URL"), item.get("url")),
    );
    if is_truthy(item.get("abstract")) {
        out.insert(
            "abstractNote".to_string(),
            item.get("abstract").unwrap().clone(),
        );
    }
    push_string_field(&mut out, "publicationTitle", item.get("container-title"));
    for (from, to) in [
        ("volume", "volume"),
        ("issue", "issue"),
        ("page", "pages"),
        ("publisher", "publisher"),
        ("language", "language"),
    ] {
        push_string_field(&mut out, to, item.get(from));
    }
    for key in ["ISSN", "ISBN"] {
        if let Some(value) = item.get(key).filter(|value| is_truthy(Some(value))) {
            out.insert(key.to_string(), Value::String(identifier_list(value)));
        }
    }
    let date = issued_to_date(item.get("issued"));
    if !date.is_empty() {
        out.insert("date".to_string(), Value::String(date));
    }
    let creators = authors_to_creators(first_truthy(item.get("author"), item.get("editor")));
    if !creators.is_empty() {
        out.insert("creators".to_string(), Value::Array(creators));
    }
    let tags = item
        .get("keyword")
        .filter(|value| is_truthy(Some(value)))
        .or_else(|| {
            item.get("categories")
                .filter(|value| is_truthy(Some(value)))
        })
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(|value| {
                    let text = value.as_str()?;
                    let mut tag = Map::new();
                    tag.insert("tag".to_string(), Value::String(text.to_string()));
                    Some(Value::Object(tag))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if !tags.is_empty() {
        out.insert("tags".to_string(), Value::Array(tags));
    }
    out
}
