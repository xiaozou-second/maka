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

use crate::host_support as host;

use futures_util::FutureExt;
use maka_event_log::root::ROOT_MARKER;
use maka_remote_pairing::{DeviceIdentity, HOST_IDENTITY_FILE, SignedPairingTicket};
use serde_json::Value;
use std::{
    fs,
    panic::AssertUnwindSafe,
    time::{SystemTime, UNIX_EPOCH},
};

#[tokio::test]
async fn peer_access_uses_the_running_hosts_identity_and_observation_deadline() {
    let sandbox = host::Sandbox::new().await.unwrap();
    let executable = crate::maka_executable();
    let mut host = host::Host::spawn_with(
        &executable,
        &sandbox.root,
        &sandbox.home,
        &sandbox.workspace,
    )
    .unwrap();
    let result = AssertUnwindSafe(async {
        let (mut client, _) = host.ready().await.unwrap();
        let backup = sandbox.directory.path().join("backup");
        fs::create_dir(&backup).unwrap();
        fs::copy(sandbox.root.join(ROOT_MARKER), backup.join(ROOT_MARKER)).unwrap();
        let host_key = DeviceIdentity::load_or_create(&sandbox.root.join(HOST_IDENTITY_FILE))
            .unwrap()
            .public_key();
        let backup_key = DeviceIdentity::load_or_create(&backup.join(HOST_IDENTITY_FILE))
            .unwrap()
            .public_key();
        assert_ne!(host_key, backup_key);

        let command = || host::command_for(&executable, &sandbox.home, &sandbox.workspace);
        let output = command()
            .args([
                "access",
                "peer-prepare",
                "--timeout-ms",
                "15000",
                "--host-name",
                "Live Host",
                "--direct-endpoint",
                "ws://127.0.0.2:8989",
                "--root",
            ])
            .arg(&backup)
            .output()
            .await
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let invite: Value = serde_json::from_slice(&output.stdout).unwrap();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis()
            .try_into()
            .unwrap();
        let ticket =
            SignedPairingTicket::parse_and_verify_uri(invite["uri"].as_str().unwrap(), now)
                .unwrap();
        assert_eq!(ticket.payload().host_peer_id, host_key.to_string());

        let list = command()
            .args(["access", "peer-list", "--timeout-ms", "15000", "--root"])
            .arg(&backup)
            .output()
            .await
            .unwrap();
        assert!(
            list.status.success(),
            "{}",
            String::from_utf8_lossy(&list.stderr)
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&list.stdout).unwrap(),
            serde_json::json!([])
        );

        let expired = command()
            .args(["access", "peer-list", "--timeout-ms", "1", "--root"])
            .arg(&backup)
            .output()
            .await
            .unwrap();
        let diagnostic = String::from_utf8_lossy(&expired.stderr);
        assert!(
            expired.status.code() == Some(70) || diagnostic.contains("observation timed out"),
            "{diagnostic}"
        );
        host.quit(&mut client).await.unwrap();
    })
    .catch_unwind()
    .await;
    let cleanup = host.cleanup().await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
    cleanup.unwrap();
}
