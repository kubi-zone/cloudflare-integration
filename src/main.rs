mod cloudflare;

use std::{sync::Arc, time::Duration};

use clap::{Parser, Subcommand, ValueEnum};
use cloudflare::{CloudFlare, CloudFlareZone};
use futures::StreamExt as _;
use kube::{
    runtime::{controller::Action, watcher, Controller},
    Api, Client as KubeClient, ResourceExt as _,
};
use kubizone_crds::v1alpha1::DomainExt;
use kubizone_crds::v1alpha1::Zone;
use tokio::sync::watch::Receiver;
use tracing::{debug, error, info, warn};

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
        #[arg(value_enum, env, long, default_value = "Mode::Upsert")]
        mode: Mode,
    },
}

#[derive(ValueEnum, Default, Debug, Clone)]
pub enum Mode {
    #[default]
    Upsert,
    Delete,
}

struct Context {
    cloudflare: CloudFlare,
    requeue_time: Duration,
    cf_domains: Receiver<Vec<CloudFlareZone>>,
    mode: Mode,
}

async fn reconcile(zone: Arc<Zone>, ctx: Arc<Context>) -> Result<Action, kube::Error> {
    let Some(fqdn) = zone.fqdn() else {
        debug!("zone {zone} does not yet have a fully qualified domain name");
        return Ok(Action::requeue(ctx.requeue_time));
    };

    let zones = ctx.cf_domains.borrow().to_vec();

    let Some(cloudflare_zone) = zones
        .iter()
        .find(|cf_zone| cf_zone.name == fqdn.to_string().trim_end_matches('.'))
    else {
        error!(
            "zone {zone}'s fqdn {fqdn} does not match any zones in {}",
            zones
                .iter()
                .map(|zone| zone.name.clone())
                .collect::<Vec<_>>()
                .join(", ")
        );
        return Ok(Action::requeue(ctx.requeue_time));
    };

    let records = ctx.cloudflare.records(&cloudflare_zone.id).await;

    println!("{records:#?}");

    Ok(Action::requeue(ctx.requeue_time))
}

fn error_policy(zone: Arc<Zone>, error: &kube::Error, _ctx: Arc<Context>) -> Action {
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

                let zones = cf_clone.list_zones().await;

                println!("{zones:#?}");
                tx.send(zones).unwrap();

                tokio::time::sleep(std::time::Duration::from_secs(60 * 5)).await;
            });

            rx.changed().await.unwrap();

            let context = Context {
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
