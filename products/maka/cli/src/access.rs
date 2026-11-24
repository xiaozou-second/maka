/*
 * Licensed to the Apache Software Foundation (ASF) under one
 * or more contributor license agreements.  See the NOTICE file
 * distributed with this work for additional information
 * regarding copyright ownership.  The ASF licenses this file
 * to you under the Apache License, Version 2.0 (the
 * "License"); you may not use this file except in compliance
 * with the License.  You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing,
 * software distributed under the License is distributed on an
 * "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
 * KIND, either express or implied.  See the License for the
 * specific language governing permissions and limitations
 * under the License.
 */

use crate::{args::Root, host_client::HostClient};
use clap::{Args, Subcommand};
pub(super) use maka_host_runtime::application::host::Pairing;
use maka_host_runtime::{
    HostError,
    application::host::{PeerId, PeerPairingOptions},
};
use maka_remote_pairing::RelayEndpoint;

#[derive(Subcommand)]
pub(super) enum Access {
    /// Print short-lived remote application pairing JSON, including its secret credential.
    Prepare(Prepare),
    /// Revoke a credential and disconnect its remote clients.
    Revoke(Revoke),
}

#[derive(Subcommand)]
pub(super) enum PeerAccess {
    /// Print a signed, one-time invitation for an authenticated native transport.
    /// The URI contains a secret; share it only with the intended recipient.
    #[command(name = "peer-prepare")]
    Prepare(PeerPrepare),
    /// List authorized native peers.
    #[command(name = "peer-list")]
    List(Root),
    /// Revoke a native peer and disconnect its sessions.
    #[command(name = "peer-revoke")]
    Revoke(PeerRevoke),
}

#[derive(Args)]
pub(super) struct Prepare {
    #[command(flatten)]
    root: Root,
    #[arg(long)]
    principal: String,
}

#[derive(Args)]
pub(super) struct Revoke {
    #[command(flatten)]
    root: Root,
    #[arg(long)]
    credential_id: String,
}

impl Access {
    pub async fn run(self) -> Result<(), HostError> {
        match self {
            Self::Prepare(args) => args.run().await,
            Self::Revoke(args) => args.run().await,
        }
    }
}

impl PeerAccess {
    pub async fn run(self) -> Result<(), HostError> {
        match self {
            Self::Prepare(args) => args.run().await,
            Self::List(root) => {
                let client = HostClient::connect(&root.root, None).await?;
                println!(
                    "{}",
                    serde_json::to_string(&client.list_peer_access().await?)?
                );
                Ok(())
            }
            Self::Revoke(args) => {
                let client = HostClient::connect(&args.root.root, None).await?;
                let revoked = client
                    .revoke_peer_access(&PeerId::new(args.peer_id)?)
                    .await?;
                println!("{}", serde_json::json!({"revoked": revoked}));
                Ok(())
            }
        }
    }
}
impl Prepare {
    async fn run(self) -> Result<(), HostError> {
        let principal = pairing_input(self.principal)?;
        let mut client = HostClient::connect(&self.root.root, None).await?;
        println!(
            "{}",
            serde_json::to_string(&prepare(&mut client, principal).await?)?
        );
        Ok(())
    }
}
pub(super) fn pairing_input(principal: String) -> Result<String, HostError> {
    if principal.is_empty()
        || principal.len() > 128
        || !principal
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.:-".contains(&byte))
    {
        return Err("principal must contain 1–128 ASCII letters, digits, or _ . : -".into());
    }
    Ok(principal)
}
pub(super) async fn prepare(
    client: &mut HostClient,
    principal: String,
) -> Result<Pairing, HostError> {
    client.prepare_access(principal).await
}
impl Revoke {
    async fn run(self) -> Result<(), HostError> {
        let client = HostClient::connect(&self.root.root, None).await?;
        println!(
            "{}",
            serde_json::json!({"revoked": client.revoke_access(self.credential_id).await?})
        );
        Ok(())
    }
}

#[derive(Args)]
pub(super) struct PeerPrepare {
    #[command(flatten)]
    root: Root,
    #[arg(long)]
    label: Option<String>,
    #[arg(long)]
    ttl_secs: Option<u64>,
    #[arg(long)]
    host_name: String,
    #[arg(long)]
    relay: Vec<String>,
    /// QUIC port when specifying a single relay.
    #[arg(long, requires = "relay")]
    relay_quic_port: Option<u16>,
    /// SHA-256 certificate fingerprint when specifying a single relay.
    #[arg(long, requires = "relay")]
    relay_cert_sha256: Option<String>,
    #[arg(long)]
    direct_endpoint: Vec<String>,
}

#[derive(Args)]
pub(super) struct PeerRevoke {
    #[command(flatten)]
    root: Root,
    #[arg(long)]
    peer_id: String,
}

impl PeerPrepare {
    async fn run(self) -> Result<(), HostError> {
        if self.relay.len() > 1
            && (self.relay_quic_port.is_some() || self.relay_cert_sha256.is_some())
        {
            return Err("relay QUIC port and certificate pin require a single relay".into());
        }
        let relays: Vec<_> = self
            .relay
            .into_iter()
            .map(|url| RelayEndpoint {
                url,
                quic_port: self.relay_quic_port,
                cert_sha256: self.relay_cert_sha256.clone(),
            })
            .collect();
        let client = HostClient::connect(&self.root.root, None).await?;
        let invite = client
            .prepare_peer_pairing(PeerPairingOptions {
                host_name: self.host_name,
                relays,
                direct_endpoints: self.direct_endpoint,
                label: self.label,
                ttl_secs: self.ttl_secs,
            })
            .await?;
        println!("{}", serde_json::to_string(&invite)?);
        Ok(())
    }
}
