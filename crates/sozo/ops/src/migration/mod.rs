use std::str::FromStr;
use std::sync::Arc;

use anyhow::{anyhow, bail, Result};
use dojo_world::contracts::naming::compute_selector_from_tag;
use dojo_world::contracts::WorldContract;
use dojo_world::manifest::{BASE_DIR, MANIFESTS_DIR, OVERLAYS_DIR};
use dojo_world::metadata::get_default_namespace_from_ws;
use dojo_world::migration::world::WorldDiff;
use dojo_world::migration::{DeployOutput, TxnConfig, UpgradeOutput};
use dojo_world::utils::{TransactionExt, TransactionWaiter};
use scarb::core::Workspace;
use starknet::accounts::{Call, ConnectedAccount, ExecutionEncoding, SingleOwnerAccount};
use starknet::core::types::{Felt, InvokeTransactionResult};
use starknet::core::utils::{cairo_short_string_to_felt, get_contract_address};
use starknet::macros::selector;
use starknet::providers::jsonrpc::HttpTransport;
use starknet::providers::{AnyProvider, JsonRpcClient, Provider};
use starknet::signers::{LocalWallet, SigningKey};
use starknet_crypto::poseidon_hash_single;
use url::Url;

mod auto_auth;
mod migrate;
pub mod ui;
mod utils;

pub use self::auto_auth::auto_authorize;
use self::migrate::update_manifests_and_abis;
pub use self::migrate::{
    apply_diff, execute_strategy, find_authorization_diff, prepare_migration, print_strategy,
    upload_metadata,
};
use self::ui::MigrationUi;

#[derive(Debug, Default, Clone)]
pub struct MigrationOutput {
    pub world_address: Felt,
    pub world_tx_hash: Option<Felt>,
    pub world_block_number: Option<u64>,
    // Represents if full migration got completeled.
    // If false that means migration got partially completed.
    pub full: bool,

    pub models: Vec<String>,
    pub contracts: Vec<Option<ContractMigrationOutput>>,
}

#[derive(Debug, Default, Clone)]
pub struct ContractMigrationOutput {
    pub tag: String,
    pub contract_address: Felt,
    pub base_class_hash: Felt,
    pub was_upgraded: bool,
}

/// Get predeployed accounts from the Katana RPC server.
async fn get_declarers_accounts<A: ConnectedAccount>(
    migrator: A,
    rpc_url: &str,
) -> Result<Vec<SingleOwnerAccount<AnyProvider, LocalWallet>>> {
    let client = reqwest::Client::new();
    let response = client
        .post(rpc_url)
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": "katana_predeployedAccounts",
            "params": [],
            "id": 1
        }))
        .send()
        .await;

    if response.is_err() {
        return Ok(vec![]);
    }

    let result: serde_json::Value = response.unwrap().json().await?;

    let mut declarers = vec![];

    if let Some(vals) = result.get("result").and_then(|v| v.as_array()) {
        let chain_id = migrator.provider().chain_id().await?;

        for a in vals {
            let address = a["address"].as_str().unwrap();

            // On slot, some accounts are hidden, we skip them.
            let private_key = if let Some(pk) = a["privateKey"].as_str() {
                pk
            } else {
                continue;
            };

            let provider = AnyProvider::JsonRpcHttp(JsonRpcClient::new(HttpTransport::new(
                Url::parse(rpc_url).unwrap(),
            )));

            let signer = LocalWallet::from(SigningKey::from_secret_scalar(
                Felt::from_hex(private_key).unwrap(),
            ));

            let account = SingleOwnerAccount::new(
                provider,
                signer,
                Felt::from_hex(address).unwrap(),
                chain_id,
                ExecutionEncoding::New,
            );

            declarers.push(account);
        }
    }

    Ok(declarers)
}

#[allow(clippy::too_many_arguments)]
pub async fn migrate<A>(
    ws: &Workspace<'_>,
    world_address: Option<Felt>,
    rpc_url: String,
    account: A,
    name: &str,
    dry_run: bool,
    txn_config: TxnConfig,
    skip_manifests: Option<Vec<String>>,
) -> Result<Option<MigrationOutput>>
where
    A: ConnectedAccount + Sync + Send + 'static,
    A::Provider: Send,
    A::SignError: 'static,
{
    let ui = ws.config().ui();

    // its path to a file so `parent` should never return `None`
    let root_dir = ws.manifest_path().parent().unwrap().to_path_buf();

    let profile_name =
        ws.current_profile().expect("Scarb profile expected to be defined.").to_string();
    let manifest_dir = root_dir.join(MANIFESTS_DIR).join(&profile_name);
    let manifest_base_dir = manifest_dir.join(BASE_DIR);
    let overlay_dir = root_dir.join(OVERLAYS_DIR).join(&profile_name);

    let target_dir = ws.target_dir().path_existent().unwrap();
    let target_dir = target_dir.join(ws.config().profile().as_str());

    let default_namespace = get_default_namespace_from_ws(ws)?;

    // Load local and remote World manifests.
    let (local_manifest, remote_manifest) = utils::load_world_manifests(
        &manifest_base_dir,
        &overlay_dir,
        &account,
        world_address,
        &ui,
        skip_manifests,
    )
    .await
    .map_err(|e| {
        ui.error(e.to_string());
        anyhow!(
            "\n Use `sozo clean` to clean your project.\nThen, rebuild your project with `sozo \
             build`.",
        )
    })?;

    let generated_world_address = get_world_address(&local_manifest, name)?;
    if let Some(world_address) = world_address {
        if world_address != generated_world_address {
            bail!(format!(
                "Calculated world address ({:#x}) doesn't match provided world address. If you \
                 are deploying with custom seed make sure `world_address` is correctly configured \
                 (or not set) in your `dojo_{profile_name}.toml`",
                generated_world_address
            ))
        }
    }

    // Calculate diff between local and remote World manifests.
    ui.print_step(2, "🧰", "Evaluating Worlds diff...");
    let diff =
        WorldDiff::compute(local_manifest.clone(), remote_manifest.clone(), &default_namespace)?;

    let total_diffs = diff.count_diffs();
    ui.print_sub(format!("Total diffs found: {total_diffs}"));

    if total_diffs == 0 {
        ui.print("\n✨ No diffs found. Remote World is already up to date!");
    }

    let strategy = prepare_migration(&target_dir, diff.clone(), name, world_address, &ui)?;
    // TODO: dry run can also show the diffs for things apart from world state
    // what new authorizations would be granted, if ipfs data would change or not,
    // etc...
    if dry_run {
        if total_diffs == 0 {
            return Ok(None);
        }

        print_strategy(&ui, account.provider(), &strategy, strategy.world_address).await;

        update_manifests_and_abis(
            ws,
            local_manifest,
            &manifest_dir,
            &profile_name,
            &rpc_url,
            strategy.world_address,
            None,
            name,
        )
        .await?;

        Ok(None)
    } else {
        let declarers = get_declarers_accounts(&account, &rpc_url).await?;

        let declarers_len = if declarers.is_empty() { 1 } else { declarers.len() };
        ui.print_sub(format!("Declarers: {}", declarers_len));

        let migration_output = if total_diffs != 0 {
            match apply_diff(ws, &account, txn_config, &strategy, &declarers).await {
                Ok(migration_output) => Some(migration_output),
                Err(e) => {
                    update_manifests_and_abis(
                        ws,
                        local_manifest,
                        &manifest_dir,
                        &profile_name,
                        &rpc_url,
                        strategy.world_address,
                        None,
                        name,
                    )
                    .await?;
                    return Err(e)?;
                }
            }
        } else {
            None
        };

        update_manifests_and_abis(
            ws,
            local_manifest.clone(),
            &manifest_dir,
            &profile_name,
            &rpc_url,
            strategy.world_address,
            migration_output.clone(),
            name,
        )
        .await?;

        let account = Arc::new(account);
        let world = WorldContract::new(strategy.world_address, account.clone());

        ui.print(" ");
        ui.print_step(6, "🖋️", "Authorizing systems based on overlay...");
        let (grant, revoke) = find_authorization_diff(
            &ui,
            &world,
            &diff,
            migration_output.as_ref(),
            &default_namespace,
        )
        .await?;

        match auto_authorize(ws, &world, &txn_config, &default_namespace, &grant, &revoke).await {
            Ok(()) => {
                ui.print_sub("Auto authorize completed successfully");
            }
            Err(e) => {
                ui.print_sub(format!("Failed to auto authorize with error: {e}"));
            }
        };

        if let Some(migration_output) = &migration_output {
            ui.print(" ");
            ui.print_step(7, "🏗️", "Initializing contracts...");

            // Run dojo inits now that everything is actually deployed and permissioned.
            let mut init_calls = vec![];
            for c in strategy.contracts {
                let was_upgraded = migration_output
                    .contracts
                    .iter()
                    .flatten()
                    .find(|output| output.tag == c.diff.tag)
                    .map(|output| output.was_upgraded)
                    .unwrap_or(false);

                if was_upgraded {
                    continue;
                }

                let contract_selector = compute_selector_from_tag(&c.diff.tag);
                let init_calldata: Vec<Felt> = c
                    .diff
                    .init_calldata
                    .iter()
                    .map(|s| Felt::from_str(s))
                    .collect::<Result<Vec<_>, _>>()?;

                let mut calldata = vec![contract_selector, Felt::from(init_calldata.len())];
                calldata.extend(init_calldata);

                init_calls.push(Call {
                    calldata,
                    selector: selector!("init_contract"),
                    to: strategy.world_address,
                });
            }

            if !init_calls.is_empty() {
                let InvokeTransactionResult { transaction_hash } = account
                    .execute_v1(init_calls)
                    .send_with_cfg(&TxnConfig::init_wait())
                    .await
                    .map_err(|e| {
                        ui.verbose(format!("{e:?}"));
                        anyhow!("Failed to deploy contracts: {e}")
                    })?;

                TransactionWaiter::new(transaction_hash, account.provider()).await?;
                ui.print_sub(format!("All contracts are initialized at: {transaction_hash:#x}\n"));
            } else {
                ui.print_sub("No contracts to initialize");
            }
        }

        if let Some(migration_output) = &migration_output {
            if !ws.config().offline() {
                upload_metadata(ws, &account, migration_output.clone(), txn_config).await?;
            }
        }

        Ok(migration_output)
    }
}

fn get_world_address(
    local_manifest: &dojo_world::manifest::BaseManifest,
    name: &str,
) -> Result<Felt> {
    let name = cairo_short_string_to_felt(name)?;
    let salt = poseidon_hash_single(name);

    let generated_world_address = get_contract_address(
        salt,
        local_manifest.world.inner.original_class_hash,
        &[local_manifest.base.inner.class_hash],
        Felt::ZERO,
    );

    Ok(generated_world_address)
}

#[allow(dead_code)]
enum ContractDeploymentOutput {
    AlreadyDeployed(Felt),
    Output(DeployOutput),
}

#[allow(dead_code)]
enum ContractUpgradeOutput {
    Output(UpgradeOutput),
}
