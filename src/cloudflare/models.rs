use std::{fmt::Display, hash::Hash};

use kubizone_common::{DomainSegment, FullyQualifiedDomainName, RecordIdent, Type};
use serde::{Deserialize, Serialize};
use tracing::trace;

#[derive(Debug, Deserialize, Serialize)]
#[serde(transparent)]
pub struct RecordId(String);

impl Display for RecordId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug, Deserialize)]
#[serde(from = "InternalRecord")]
pub struct Record {
    pub id: RecordId,
    pub fqdn: FullyQualifiedDomainName,
    pub r#type: Type,
    pub rdata: String,
    pub comment: Option<String>,
    pub tags: Vec<String>,
    pub ttl: u32,
}

impl From<&Record> for RecordIdent {
    fn from(value: &Record) -> Self {
        RecordIdent {
            fqdn: value.fqdn.clone(),
            r#type: value.r#type,
            rdata: value.rdata.clone(),
        }
    }
}

impl Hash for Record {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.fqdn.hash(state);
        self.r#type.hash(state);
        self.rdata.hash(state);
    }
}

impl Record {
    pub fn is_managed_by(&self, controller_name: &str) -> bool {
        let tag = format!("managed-by:{controller_name}");
        self.tags.contains(&tag) || self.comment == Some(tag)
    }
}

#[derive(Deserialize)]
struct InternalRecord {
    pub id: String,
    pub name: String,
    pub r#type: Type,
    pub content: String,
    #[serde(default)]
    pub comment: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub ttl: u32,
}

impl From<InternalRecord> for Record {
    fn from(record: InternalRecord) -> Self {
        let fqdn = Result::from_iter(record.name.split('.').map(DomainSegment::try_from)).unwrap();

        Record {
            id: RecordId(record.id),
            fqdn,
            r#type: record.r#type,
            rdata: record.content,
            comment: record.comment,
            tags: record.tags,
            ttl: record.ttl,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(transparent)]
pub struct ZoneId(String);

impl Display for ZoneId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(from = "InternalZone")]
pub struct Zone {
    pub id: ZoneId,
    pub fqdn: FullyQualifiedDomainName,
}

#[derive(Debug, Clone, Deserialize)]
struct InternalZone {
    pub id: ZoneId,
    pub name: String,
}

impl From<InternalZone> for Zone {
    fn from(zone: InternalZone) -> Self {
        let fqdn = Result::from_iter(zone.name.split('.').map(DomainSegment::try_from)).unwrap();

        Zone { id: zone.id, fqdn }
    }
}

#[derive(Debug, Clone, Deserialize, thiserror::Error)]
pub struct ApiError {
    pub code: u32,
    pub message: String,
}

impl Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Message {
    pub code: u32,
    pub message: String,
}

impl Display for Message {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(from = "InternalApiResult<T>")]
pub enum ApiResult<T> {
    Success { result: T, messages: Vec<Message> },
    Error { errors: Vec<ApiError> },
}

impl<T> ApiResult<T> {
    pub fn into_result(self) -> Result<T, ApiError> {
        match self {
            ApiResult::Success { result, messages } => {
                if !messages.is_empty() {
                    trace!(
                        "unpacking ApiResult, but dropping messages: {}",
                        messages
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }
                Ok(result)
            }
            ApiResult::Error { mut errors } => Err(errors.pop().unwrap()),
        }
    }
}

#[derive(Debug, Deserialize)]
struct InternalApiResult<T> {
    #[serde(default)]
    errors: Vec<ApiError>,
    #[serde(default)]
    messages: Vec<Message>,
    success: bool,
    result: Option<T>,
}

impl<'de, T: Deserialize<'de>> From<InternalApiResult<T>> for ApiResult<T> {
    fn from(value: InternalApiResult<T>) -> Self {
        if value.success {
            ApiResult::Success {
                result: value.result.unwrap(),
                messages: value.messages,
            }
        } else {
            ApiResult::Error {
                errors: value.errors,
            }
        }
    }
}

#[cfg(test)]
#[test]
fn parse() {
    let _result = serde_json::from_str::<ApiResult<Record>>(
        r#"{
            "result": {
                "content": "127.0.0.1",
                "name": "kubi.zone",
                "ttl": 300,
                "type": "A",
                "id": "023e105f4ecef8ad9ca31a8372d0c353",
                "tags": []
            },
            "success": true,
            "errors": [],
            "messages": []
        }"#,
    )
    .unwrap();
}
