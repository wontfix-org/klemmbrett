//! Paths into a record's nested `data`, as layouts and filters name them.
//!
//! `sender.name` walks objects, `items[0]` indexes a list, and `tags[*]`
//! takes every element. `key`, `label`, and `value` on their own name the
//! record's own strings; every other path starts inside `data`.

use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Step {
    Field(String),
    Index(usize),
    Each,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataPath {
    text: String,
    steps: Vec<Step>,
}

impl DataPath {
    pub fn parse(text: &str) -> Result<Self, String> {
        let invalid = |why: &str| format!("invalid path {text:?}: {why}");
        let mut steps = Vec::new();
        let mut rest = text;
        let mut expect_field = true;
        while !rest.is_empty() {
            if let Some(after) = rest.strip_prefix('[') {
                let end = after.find(']').ok_or_else(|| invalid("missing ]"))?;
                let index = &after[..end];
                steps.push(if index == "*" {
                    Step::Each
                } else {
                    Step::Index(
                        index
                            .parse()
                            .map_err(|_| invalid("an index is a number or *"))?,
                    )
                });
                rest = &after[end + 1..];
                expect_field = false;
                continue;
            }
            if !expect_field {
                rest = rest
                    .strip_prefix('.')
                    .ok_or_else(|| invalid("expected . or [ after an index"))?;
            }
            let end = rest.find(['.', '[']).unwrap_or(rest.len());
            if end == 0 {
                return Err(invalid("empty field name"));
            }
            steps.push(Step::Field(rest[..end].to_owned()));
            rest = &rest[end..];
            if let Some(after) = rest.strip_prefix('.') {
                if after.is_empty() {
                    return Err(invalid("ends with ."));
                }
                rest = after;
                expect_field = true;
            } else {
                expect_field = false;
            }
        }
        if steps.is_empty() {
            return Err(invalid("empty"));
        }
        Ok(Self {
            text: text.to_owned(),
            steps,
        })
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// `key`, `label`, or `value` when the path is just that name.
    pub fn record_field(&self) -> Option<&str> {
        match self.steps.as_slice() {
            [Step::Field(name)] if matches!(name.as_str(), "key" | "label" | "value") => Some(name),
            _ => None,
        }
    }

    /// Every value the path reaches. Missing steps reach nothing.
    pub fn select<'a>(&self, root: &'a Value) -> Vec<&'a Value> {
        let mut current = vec![root];
        for step in &self.steps {
            let mut next = Vec::new();
            for value in current {
                match (step, value) {
                    (Step::Field(name), Value::Object(map)) => next.extend(map.get(name)),
                    (Step::Index(index), Value::Array(items)) => next.extend(items.get(*index)),
                    (Step::Each, Value::Array(items)) => next.extend(items.iter()),
                    (Step::Each, Value::Object(map)) => next.extend(map.values()),
                    _ => {}
                }
            }
            current = next;
        }
        current
    }
}

/// How a value reads in a row: strings as they are, `null` as nothing, and
/// everything else as compact JSON.
pub fn display(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn texts(path: &str, root: &Value) -> Vec<String> {
        DataPath::parse(path)
            .unwrap()
            .select(root)
            .into_iter()
            .map(display)
            .collect()
    }

    #[test]
    fn paths_walk_objects_lists_and_wildcards() {
        let root = json!({
            "sender": {"name": "Ada", "age": 36},
            "tags": ["work", "urgent"],
            "items": [{"id": 1}, {"id": 2}],
        });
        assert_eq!(texts("sender.name", &root), ["Ada"]);
        assert_eq!(texts("sender.age", &root), ["36"]);
        assert_eq!(texts("tags[1]", &root), ["urgent"]);
        assert_eq!(texts("tags[*]", &root), ["work", "urgent"]);
        assert_eq!(texts("items[*].id", &root), ["1", "2"]);
        assert_eq!(texts("sender", &root), [r#"{"name":"Ada","age":36}"#]);
        assert!(texts("missing.deeper", &root).is_empty());
        assert!(texts("tags[9]", &root).is_empty());
    }

    #[test]
    fn record_fields_are_recognized_only_alone() {
        assert_eq!(
            DataPath::parse("label").unwrap().record_field(),
            Some("label")
        );
        assert_eq!(DataPath::parse("label.x").unwrap().record_field(), None);
        assert_eq!(DataPath::parse("summary").unwrap().record_field(), None);
    }

    #[test]
    fn malformed_paths_are_rejected() {
        for bad in ["", "a..b", "a.", "[0]x", "a[", "a[x]", ".a"] {
            assert!(DataPath::parse(bad).is_err(), "{bad:?} should be rejected");
        }
        assert!(DataPath::parse("[0]").is_ok());
        assert!(DataPath::parse("a[0][1].b").is_ok());
    }
}
