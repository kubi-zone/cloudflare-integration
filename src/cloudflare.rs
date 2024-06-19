use k8s_openapi::serde::Deserialize;
use kubizone_crds::kubizone_common::Type;
use reqwest::{
    header::{HeaderMap, HeaderValue, AUTHORIZATION},
    Client,
};

#[derive(Debug, Clone)]
pub struct CloudFlare {
    client: Client,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CloudFlareZone {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CloudFlareRecord {
    pub name: String,
    pub rdata: String,
    pub r#type: Type,
    pub tags: Vec<String>,
    pub id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Message {
    pub code: u32,
    pub message: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApiResult<T> {
    pub errors: Vec<Message>,
    pub messages: Vec<Message>,
    pub success: bool,
    pub result: Vec<T>,
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

    pub async fn list_zones(&self) -> Vec<CloudFlareZone> {
        let response: ApiResult<CloudFlareZone> = self
            .client
            .get("https://api.cloudflare.com/client/v4/zones")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();

        response.result
    }

    pub async fn records(&self, zone: &str) -> Vec<CloudFlareRecord> {
        let response: ApiResult<CloudFlareRecord> = self
            .client
            .get(format!(
                "https://api.cloudflare.com/client/v4/zones/{zone}/dns_records"
            ))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();

        response.result
    }
}
