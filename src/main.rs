mod cloudflare;

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use clap::{Parser, Subcommand, ValueEnum};
use cloudflare::CloudFlare;
use futures::StreamExt as _;
use kube::{
    runtime::{controller::Action, watcher, Controller},
    Api, Client as KubeClient, ResourceExt as _,
};
use kubizone_common::{FullyQualifiedDomainName, RecordIdent};
use kubizone_crds::v1alpha1::DomainExt;
use kubizone_crds::v1alpha1::Zone;
use tokio::sync::watch::Receiver;
use tracing::{debug, error, info, info_span, trace, warn};

#[derive(Debug, Parser)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Reconcile {
        #[arg(env, long, default_value_t = 30)]
        requeue_time_secs: u64,
        #[arg(env, long)]
        cf_api_key: String,
        #[arg(value_enum, env, long, default_value_t = Mode::Upsert)]
        mode: Mode,
    },
}

#[derive(ValueEnum, Default, Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Upsert,
    Delete,
}

struct Context {
    controller_name: String,
    cloudflare: CloudFlare,
    requeue_time: Duration,
    cf_domains: Receiver<Vec<cloudflare::Zone>>,
    mode: Mode,
}

impl Context {
    pub fn find_cloudflare_zone(
        &self,
        fqdn: &FullyQualifiedDomainName,
    ) -> Result<cloudflare::Zone, Error> {
        let managed_zones = self.cf_domains.borrow();

        let Some(zone) = managed_zones
            .iter()
            .find(|zone| &zone.fqdn == fqdn)
            .cloned()
        else {
            warn!(
                "{fqdn} does not match any zones in {}",
                managed_zones
                    .iter()
                    .map(|zone| zone.fqdn.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            );

            return Err(Error::ZoneNotFound(fqdn.clone()));
        };

        Ok(zone)
    }
}

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error("cloudflare: {0}")]
    CloudFlare(#[from] cloudflare::Error),
    #[error("kube: {0}")]
    Kube(#[from] kube::Error),
    #[error("zone not found in cloudflare: {0}")]
    ZoneNotFound(FullyQualifiedDomainName),
    #[error("zone has no entries: {0}")]
    ZoneHasNoEntries(String),
}

async fn reconcile(zone: Arc<Zone>, ctx: Arc<Context>) -> Result<Action, Error> {
    let Some(fqdn) = zone.fqdn() else {
        debug!("zone {zone} does not yet have a fully qualified domain name");
        return Ok(Action::requeue(ctx.requeue_time));
    };

    let cloudflare_zone = ctx.find_cloudflare_zone(&fqdn)?;

    // Collect all existing entries in (RecordIdent, Record) map.
    let records = ctx
        .cloudflare
        .records(&cloudflare_zone.id)
        .await?
        .into_iter()
        .map(|record| (RecordIdent::from(&record), record))
        .collect::<HashMap<_, _>>();

    // Collect all desired entries in (RecordIdent, ZoneEntry) map.
    let entries = zone
        .status
        .as_ref()
        .map(|status| &status.entries)
        .ok_or(Error::ZoneHasNoEntries(zone.name_any()))?
        .into_iter()
        .filter(|entry| !entry.type_.is_soa())
        .map(|entry| (RecordIdent::from(entry), entry))
        .collect::<HashMap<_, _>>();

    // Create missing entries
    for missing_entry in entries
        .iter()
        .filter_map(|(ident, entry)| (!records.contains_key(ident)).then_some(entry))
    {
        info_span!("create");

        info!(
            "creating record {missing_entry:?} in {} with value {}",
            cloudflare_zone.fqdn, missing_entry.rdata
        );

        ctx.cloudflare
            .create_record(&cloudflare_zone.id, &ctx.controller_name, missing_entry)
            .await?;
    }

    // Delete unexpected records (that we manage)
    for (ident, unexpected_record) in records
        .iter()
        .filter(|(ident, _)| !entries.contains_key(ident))
    {
        info_span!("delete");
        if !unexpected_record.is_managed_by(&ctx.controller_name) {
            debug!("unexpected record {ident:?} found in zone {cloudflare_zone:?} has no corresponding entry in zone {zone}, but record is not managed by us.");
            continue;
        }

        if ctx.mode == Mode::Delete {
            info!(
                "deleting record {ident:?} in {} with id {}",
                cloudflare_zone.fqdn, unexpected_record.id
            );
            ctx.cloudflare
                .delete_record(&cloudflare_zone.id, &unexpected_record.id)
                .await?;
        } else {
            info!("not deleting {ident:?}, since controller is running in 'upsert' mode");
        }
    }

    // Update records (that we manage) with new information
    for (ident, entry, record) in entries
        .keys()
        .collect::<HashSet<_>>()
        .intersection(&records.keys().collect::<HashSet<_>>())
        .filter_map(|ident| Some((ident, entries.get(ident)?, records.get(ident)?)))
    {
        info_span!("update");

        if !record.is_managed_by(&ctx.controller_name) {
            info!("entry {ident:?} appears in zone {zone}, but the corresponding record in cloudflare is not managed by us");
            continue;
        }

        if entry.rdata == record.rdata && entry.ttl == record.ttl {
            trace!("record {ident:?} already up to date");
            continue;
        }

        // Update record.
        info!(
            "updating record {ident:?} in {} from {} with ttl {} => {} with ttl {}",
            cloudflare_zone.fqdn, entry.rdata, entry.ttl, record.rdata, record.ttl
        );

        ctx.cloudflare
            .update_record(&cloudflare_zone.id, &record.id, entry)
            .await?;
    }

    Ok(Action::requeue(ctx.requeue_time))
}

fn error_policy(zone: Arc<Zone>, error: &Error, _ctx: Arc<Context>) -> Action {
    error!(
        "zone {} reconciliation encountered error: {error}",
        zone.name_any()
    );
    Action::requeue(Duration::from_secs(60))
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    tracing_subscriber::fmt::init();

    let args = Args::parse();

    let client = KubeClient::try_default().await.unwrap();

    match args.command {
        Command::Reconcile {
            requeue_time_secs,
            cf_api_key,
            mode,
        } => {
            let cloudflare = CloudFlare::new(&cf_api_key);

            let (tx, mut rx) = tokio::sync::watch::channel(vec![]);

            let cf_clone = cloudflare.clone();
            tokio::spawn(async move {
                //let mut domains = tx;

                if let Ok(zones) = cf_clone.list_zones().await {
                    println!("new zones just dropped! {zones:#?}");
                    tx.send(zones).unwrap();
                }

                tokio::time::sleep(std::time::Duration::from_secs(60 * 5)).await;
            });

            rx.changed().await.unwrap();

            let context = Context {
                controller_name: "kubizone-cloudflare".to_string(),
                requeue_time: std::time::Duration::from_secs(requeue_time_secs),
                cloudflare,
                cf_domains: rx,
                mode,
            };

            let zones = Api::<Zone>::all(client.clone());

            Controller::new(zones.clone(), watcher::Config::default())
                .shutdown_on_signal()
                .run(reconcile, error_policy, Arc::new(context))
                .for_each(|res| async move {
                    match res {
                        Ok(o) => info!("reconciled: {:?}", o),
                        Err(e) => warn!("reconciliation failed: {}", e),
                    }
                })
                .await;
        }
    };
}
