//! Models related to Uptime Kuma tags

use crate::deserialize::DeserializeNumberLenient;
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, skip_serializing_none};

#[skip_serializing_none]
#[serde_as]
#[derive(Clone, Default, Debug, PartialEq, Serialize, Deserialize, Hash, Eq)]
pub struct TagDefinition {
    #[serde(rename = "id")]
    #[serde_as(as = "Option<DeserializeNumberLenient>")]
    pub tag_id: Option<i32>,

    #[serde(rename = "name")]
    pub name: Option<String>,

    #[serde(rename = "color")]
    pub color: Option<String>,
}

#[skip_serializing_none]
#[serde_as]
#[derive(Clone, Default, Debug, PartialEq, Serialize, Deserialize, Hash, Eq)]
pub struct Tag {
    #[serde(rename = "tag_id")]
    #[serde_as(as = "Option<DeserializeNumberLenient>")]
    pub tag_id: Option<i32>,

    #[serde(rename = "name")]
    pub name: Option<String>,

    #[serde(rename = "color")]
    pub color: Option<String>,

    #[serde(rename = "value")]
    pub value: Option<String>,
}

impl From<TagDefinition> for Tag {
    fn from(value: TagDefinition) -> Self {
        Tag {
            name: value.name,
            color: value.color,
            tag_id: value.tag_id,
            value: None,
        }
    }
}

impl From<Tag> for TagDefinition {
    fn from(value: Tag) -> Self {
        TagDefinition {
            tag_id: value.tag_id,
            name: value.name,
            color: value.color,
        }
    }
}

#[cfg(feature = "private-api")]
#[skip_serializing_none]
#[serde_as]
#[derive(Clone, Default, Debug, PartialEq, Serialize, Deserialize, Hash, Eq)]
pub struct TagValue {
    #[serde(rename = "name")]
    pub name: String,

    #[serde(rename = "value")]
    pub value: Option<String>,
}

/// Deserializes tag references by Uptime Kuma tag name, either as a
/// comma-separated string (`"Containers, Pihole:primary"`, `name:value`) or as
/// a list of strings / `TagValue` objects.
#[cfg(feature = "private-api")]
pub struct DeserializeTagValuesLenient;

#[cfg(feature = "private-api")]
impl DeserializeTagValuesLenient {
    fn parse(s: &str) -> Option<TagValue> {
        let (name, value) = match s.split_once(':') {
            Some((name, value)) => (name.trim(), Some(value.trim().to_owned())),
            None => (s.trim(), None),
        };
        (!name.is_empty()).then(|| TagValue {
            name: name.to_owned(),
            value,
        })
    }
}

#[cfg(feature = "private-api")]
impl<'de> serde_with::DeserializeAs<'de, Vec<TagValue>> for DeserializeTagValuesLenient {
    fn deserialize_as<D>(deserializer: D) -> Result<Vec<TagValue>, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error;
        use serde_json::Value;

        match Value::deserialize(deserializer)? {
            Value::String(s) => Ok(s.split(',').filter_map(Self::parse).collect()),
            Value::Array(items) => items
                .into_iter()
                .filter_map(|item| match item {
                    Value::String(s) => Self::parse(&s).map(Ok),
                    other => Some(serde_json::from_value(other).map_err(D::Error::custom)),
                })
                .collect(),
            _ => Err(D::Error::custom(
                "expected a comma-separated string or a list of tags",
            )),
        }
    }
}

#[cfg(feature = "private-api")]
impl serde_with::SerializeAs<Vec<TagValue>> for DeserializeTagValuesLenient {
    fn serialize_as<S>(source: &Vec<TagValue>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        source.serialize(serializer)
    }
}

#[cfg(all(test, feature = "private-api"))]
mod tests {
    use super::*;
    use serde_json::json;

    #[serde_as]
    #[derive(Deserialize)]
    struct Wrapper(#[serde_as(as = "DeserializeTagValuesLenient")] Vec<TagValue>);

    fn tag(name: &str, value: Option<&str>) -> TagValue {
        TagValue {
            name: name.to_owned(),
            value: value.map(str::to_owned),
        }
    }

    #[test]
    fn kuma_tags_parse_string_and_list() {
        let expected = vec![tag("Containers", None), tag("Pihole", Some("primary"))];
        for input in [
            json!("Containers, Pihole:primary"),
            json!(["Containers", "Pihole:primary"]),
            json!([{"name": "Containers"}, {"name": "Pihole", "value": "primary"}]),
        ] {
            let Wrapper(parsed) = serde_json::from_value(input).unwrap();
            assert_eq!(parsed, expected);
        }
    }
}
