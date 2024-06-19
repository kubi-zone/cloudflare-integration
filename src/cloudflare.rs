use kubizone_crds::{kubizone_common::Type, v1alpha1::ZoneEntry};
use reqwest::{
    header::{HeaderMap, HeaderValue, AUTHORIZATION},
    Client, IntoUrl, Method,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use tracing::error;

pub mod models;

pub use models::*;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("reqwest: {0}")]
    Reqwest(#[from] reqwest::Error),
    #[error("api: {0}")]
    Api(#[from] ApiError),
    #[error("deserialization: {0}")]
    Deserialization(#[from] serde_json::Error),
}

#[derive(Debug, Clone)]
pub struct CloudFlare {
    client: Client,
}

impl CloudFlare {
    pub fn new(token: &str) -> Self {
        let client = Client::builder()
            .default_headers(HeaderMap::from_iter([(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
            )]))
            .build()
            .unwrap();

        CloudFlare { client }
    }

    async fn request<I, O>(&self, method: Method, url: impl IntoUrl, data: I) -> Result<O, Error>
    where
        I: Serialize,
        O: DeserializeOwned,
    {
        let body = self
            .client
            .request(method, url)
            .json(&data)
            .send()
            .await?
            .text()
            .await?;

        let result = match serde_json::from_str::<ApiResult<O>>(&body) {
            Ok(result) => result,
            Err(err) => {
                error!("failed to deserialize api result: {err}, {body}");
                return Err(Error::Deserialization(err));
            }
        };

        Ok(result.into_result()?)
    }

    pub async fn list_zones(&self) -> Result<Vec<models::Zone>, Error> {
        self.request(
            Method::GET,
            "https://api.cloudflare.com/client/v4/zones",
            (),
        )
        .await
    }

    pub async fn records(&self, zone_id: &ZoneId) -> Result<Vec<models::Record>, Error> {
        self.request(
            Method::GET,
            format!("https://api.cloudflare.com/client/v4/zones/{zone_id}/dns_records"),
            (),
        )
        .await
    }

    pub async fn create_record(
        &self,
        zone_id: &ZoneId,
        managed_by: &str,
        entry: &ZoneEntry,
    ) -> Result<models::RecordId, Error> {
        #[derive(Serialize)]
        struct CreateRecord<'a> {
            pub content: &'a str,
            pub name: &'a str,
            pub proxied: bool,
            pub r#type: Type,
            pub comment: &'a str,
            pub id: &'a str,
            pub tags: Vec<String>,
            pub zone_id: &'a ZoneId,
        }

        let result: Record = self
            .request(
                Method::POST,
                format!("https://api.cloudflare.com/client/v4/zones/{zone_id}/dns_records"),
                CreateRecord {
                    content: &entry.rdata,
                    name: &entry.fqdn.to_string(),
                    proxied: false,
                    r#type: entry.type_,
                    comment: &format!("managed-by:{managed_by}"),
                    id: "",
                    tags: vec![],
                    zone_id,
                },
            )
            .await?;

        Ok(result.id)
    }

    pub async fn update_record(
        &self,
        zone_id: &ZoneId,
        record_id: &RecordId,
        entry: &ZoneEntry,
    ) -> Result<models::Record, Error> {
        #[derive(Serialize)]
        struct UpdateRecord<'a> {
            pub content: &'a str,
            pub ttl: u32,
        }

        self.request(
            Method::PATCH,
            format!("https://api.cloudflare.com/client/v4/zones/{zone_id}/dns_records/{record_id}"),
            UpdateRecord {
                content: &entry.rdata,
                ttl: entry.ttl,
            },
        )
        .await
    }

    pub async fn delete_record(
        &self,
        zone_id: &ZoneId,
        record_id: &RecordId,
    ) -> Result<RecordId, Error> {
        #[derive(Deserialize)]
        struct DeleteSuccess {
            id: RecordId,
        }

        let response: DeleteSuccess = self
            .request(
                Method::DELETE,
                format!(
                    "https://api.cloudflare.com/client/v4/zones/{zone_id}/dns_records/{record_id}"
                ),
                (),
            )
            .await?;

        Ok(response.id)
    }
}
