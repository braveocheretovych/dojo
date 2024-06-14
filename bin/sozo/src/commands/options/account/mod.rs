use std::env::current_dir;
use std::fs;
use std::path::Path;
use std::str::FromStr;

use account_sdk::account::session::hash::{AllowedMethod, Session};
use account_sdk::account::session::SessionAccount;
use account_sdk::deploy_contract::UDC_ADDRESS;
use account_sdk::signers::HashSigner;
use anyhow::{anyhow, Result};
use camino::Utf8PathBuf;
use clap::Args;
use dojo_world::manifest::DeploymentManifest;
use dojo_world::metadata::Environment;
use scarb::core::Config;
use serde::{Deserialize, Serialize};
use slot::session::Policy;
use starknet::accounts::{ExecutionEncoding, SingleOwnerAccount};
use starknet::core::types::contract::AbiEntry;
use starknet::core::types::{BlockId, BlockTag, FieldElement};
use starknet::macros::short_string;
use starknet::providers::Provider;
use starknet::signers::{LocalWallet, SigningKey};
use tracing::{info, trace};
use url::Url;

use super::signer::SignerOptions;
use super::DOJO_ACCOUNT_ADDRESS_ENV_VAR;

mod r#type;

#[derive(Debug, Args)]
#[command(next_help_heading = "Controller options")]
pub struct ControllerOptions {
    #[arg(long = "slot.auth")]
    #[arg(global = true)]
    pub controller: bool,
}

// INVARIANT:
// - For commandline: we can either specify `private_key` or `keystore_path` along with
//   `keystore_password`. This is enforced by Clap.
// - For `Scarb.toml`: if both private_key and keystore are specified in `Scarb.toml` private_key
//   will take priority
#[derive(Debug, Args)]
#[command(next_help_heading = "Account options")]
pub struct AccountOptions {
    #[arg(long, env = DOJO_ACCOUNT_ADDRESS_ENV_VAR)]
    #[arg(global = true)]
    pub account_address: Option<FieldElement>,

    #[command(flatten)]
    #[command(next_help_heading = "Controller options")]
    pub controller: ControllerOptions,

    #[command(flatten)]
    #[command(next_help_heading = "Signer options")]
    pub signer: SignerOptions,

    #[arg(long)]
    #[arg(help = "Use legacy account (cairo0 account)")]
    #[arg(global = true)]
    pub legacy: bool,
}

impl AccountOptions {
    // build the controller session account
    // check if session key exist in `slot/session.json`
    // if not, call `slot auth create-session` to create a new session key
    //		extract all the contracts' address and methods from the deployment manifest to create the policy
    // else, load the session key from `slot/session.json`
    // create a new session account with the session key
    // return the session account
    pub async fn controller<P: Provider>(
        &self,
        provider: P,
        _: &Config,
    ) -> Result<SessionAccount<P, SigningKey, SigningKey>>
    where
        P: Send + Sync,
    {
        let chain_id = provider.chain_id().await?;

        let mut path = dirs::config_local_dir().unwrap();
        path.extend(["slot"]);

        let mut cred_path = path.clone();
        cred_path.extend(["credentials.json"]);

        info!(path = ?cred_path, "Credentials path.");

        let credentials = fs::read(cred_path)?;
        let credentials = serde_json::from_slice::<Credentials>(&credentials)?;

        let mut sesh_path = path.clone();
        sesh_path.extend([&credentials.account.id, &format!("{chain_id:#x}-session.json")]);

        info!(path = ?sesh_path, "Session path.");

        // read the deployment manifest
        let policies = {
            let current_dir = current_dir()?;
            // let root_dir = current_dir.join("examples/spawn-and-move");
            let root_dir = current_dir;
            info!(path = ?root_dir, "Root directory.");

            let content = fs::read_to_string(root_dir.join("manifests/dev/manifest.toml"))?;
            let manifest = toml::from_str::<DeploymentManifest>(&content)?;

            let mut policies = get_policies(manifest, &root_dir);

            // for declaring tx
            policies.push(Policy {
                method: "__declare_transaction__".to_string(),
                target: FieldElement::from_str(&credentials.account.contract_address)?,
            });
            // for deploying using udc tx
            policies.push(Policy { method: "deployContract".to_string(), target: *UDC_ADDRESS });
            policies
        };

        info!(policies_count = policies.len(), "Extracted policies from project.");

        let session_details = match slot::session::get(chain_id) {
            Ok(session) => session,
            // TODO(kariy): should handle non authenticated error
            Err(_) => {
                let rpc_url = Url::parse("http://localhost:5050")?;
                let session = slot::session::create(rpc_url, &policies).await?;
                session
            }
        };

        // info!(?session_details, "Session details");

        let methods = session_details
            .policies
            .into_iter()
            .map(|p| AllowedMethod::new(p.target, &p.method))
            .collect::<Result<Vec<AllowedMethod>, _>>()?;

        let address = FieldElement::from_str(&credentials.account.contract_address)?;
        let guardian = SigningKey::from_secret_scalar(short_string!("CARTRIDGE_GUARDIAN"));
        let signer = SigningKey::from_secret_scalar(session_details.credentials.private_key);

        let expires_at: u64 = session_details.expires_at.parse()?;
        let session = Session::new(methods, expires_at, &signer.signer())?;

        let session_account = SessionAccount::new(
            provider,
            signer,
            guardian,
            address,
            chain_id,
            session_details.credentials.authorization,
            session,
        );

        trace!("Created Controller session account");

        Ok(session_account)
    }

    pub async fn account<P: Provider>(
        &self,
        provider: P,
        env_metadata: Option<&Environment>,
    ) -> SingleOwnerAccount<P, LocalWallet>
    where
        P: Send + Sync,
    {
        let account_address = self.account_address(env_metadata).unwrap();
        trace!(?account_address, "Account address determined.");

        let signer = self.signer.signer(env_metadata, false).unwrap();
        trace!(?signer, "Signer obtained.");

        let chain_id = provider.chain_id().await.unwrap();
        trace!(?chain_id);

        let encoding = if self.legacy { ExecutionEncoding::Legacy } else { ExecutionEncoding::New };
        trace!(?encoding, "Creating SingleOwnerAccount.");
        let mut account =
            SingleOwnerAccount::new(provider, signer, account_address, chain_id, encoding);

        // The default is `Latest` in starknet-rs, which does not reflect
        // the nonce changes in the pending block.
        account.set_block_id(BlockId::Tag(BlockTag::Pending));
        account
    }

    pub fn account_address(&self, env_metadata: Option<&Environment>) -> Result<FieldElement> {
        if let Some(address) = self.account_address {
            trace!(?address, "Account address found.");
            Ok(address)
        } else if let Some(address) = env_metadata.and_then(|env| env.account_address()) {
            trace!(address, "Account address found in environment metadata.");
            Ok(FieldElement::from_str(address)?)
        } else {
            Err(anyhow!(
                "Could not find account address. Please specify it with --account-address or in \
                 the environment config."
            ))
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessToken {
    pub token: String,
    pub r#type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LegacyCredentials {
    access_token: String,
    token_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credentials {
    #[serde(flatten)]
    pub account: Account,
    pub access_token: AccessToken,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: String,
    pub name: Option<String>,
    pub contract_address: String,
    pub credentials: AccountCredentials,
}

#[derive(Deserialize, Debug, Clone, Serialize)]
pub struct AccountCredentials {
    pub webauthn: Option<Vec<CredentialsWebauthn>>,
}
#[derive(Deserialize, Debug, Clone, Serialize)]
pub struct CredentialsWebauthn {
    pub id: String,
    #[serde(rename = "publicKey")]
    pub public_key: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JsCredentials {
    pub authorization: Vec<FieldElement>,
    pub private_key: FieldElement,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JsPolicy {
    pub target: String,
    pub method: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JsSession {
    pub policies: Vec<JsPolicy>,
    pub expires_at: String,
    pub credentials: JsCredentials,
}

#[cfg(test)]
mod tests {
    use clap::Parser;
    use starknet::accounts::{Call, ExecutionEncoder};
    use starknet_crypto::FieldElement;

    use super::{AccountOptions, DOJO_ACCOUNT_ADDRESS_ENV_VAR};

    #[derive(clap::Parser, Debug)]
    struct Command {
        #[clap(flatten)]
        pub account: AccountOptions,
    }

    #[test]
    fn account_address_read_from_env_variable() {
        std::env::set_var(DOJO_ACCOUNT_ADDRESS_ENV_VAR, "0x0");

        let cmd = Command::parse_from([""]);
        assert_eq!(cmd.account.account_address, Some(FieldElement::from_hex_be("0x0").unwrap()));
    }

    #[test]
    fn account_address_from_args() {
        let cmd = Command::parse_from(["sozo", "--account-address", "0x0"]);
        assert_eq!(
            cmd.account.account_address(None).unwrap(),
            FieldElement::from_hex_be("0x0").unwrap()
        );
    }

    #[test]
    fn account_address_from_env_metadata() {
        let env_metadata = dojo_world::metadata::Environment {
            account_address: Some("0x0".to_owned()),
            ..Default::default()
        };

        let cmd = Command::parse_from([""]);
        assert_eq!(
            cmd.account.account_address(Some(&env_metadata)).unwrap(),
            FieldElement::from_hex_be("0x0").unwrap()
        );
    }

    #[test]
    fn account_address_from_both() {
        let env_metadata = dojo_world::metadata::Environment {
            account_address: Some("0x0".to_owned()),
            ..Default::default()
        };

        let cmd = Command::parse_from(["sozo", "--account-address", "0x1"]);
        assert_eq!(
            cmd.account.account_address(Some(&env_metadata)).unwrap(),
            FieldElement::from_hex_be("0x1").unwrap()
        );
    }

    #[test]
    fn account_address_from_neither() {
        let cmd = Command::parse_from([""]);
        assert!(cmd.account.account_address(None).is_err());
    }

    #[katana_runner::katana_test(2, true)]
    async fn legacy_flag_works_as_expected() {
        let cmd = Command::parse_from([
            "sozo",
            "--legacy",
            "--account-address",
            "0x0",
            "--private-key",
            "0x1",
        ]);
        let dummy_call = vec![Call {
            to: FieldElement::from_hex_be("0x0").unwrap(),
            selector: FieldElement::from_hex_be("0x1").unwrap(),
            calldata: vec![
                FieldElement::from_hex_be("0x2").unwrap(),
                FieldElement::from_hex_be("0x3").unwrap(),
            ],
        }];

        // HACK: SingleOwnerAccount doesn't expose a way to check `encoding` type used in struct, so
        // checking it by encoding a dummy call and checking which method it used to encode the call
        let account = cmd.account.account(runner.provider(), None).await;
        let result = account.encode_calls(&dummy_call);
        // 0x0 is the data offset.
        assert!(*result.get(3).unwrap() == FieldElement::from_hex_be("0x0").unwrap());
    }

    #[katana_runner::katana_test(2, true)]
    async fn without_legacy_flag_works_as_expected() {
        let cmd = Command::parse_from(["sozo", "--account-address", "0x0", "--private-key", "0x1"]);
        let dummy_call = vec![Call {
            to: FieldElement::from_hex_be("0x0").unwrap(),
            selector: FieldElement::from_hex_be("0x1").unwrap(),
            calldata: vec![
                FieldElement::from_hex_be("0xf2").unwrap(),
                FieldElement::from_hex_be("0xf3").unwrap(),
            ],
        }];

        // HACK: SingleOwnerAccount doesn't expose a way to check `encoding` type used in struct, so
        // checking it by encoding a dummy call and checking which method it used to encode the call
        let account = cmd.account.account(runner.provider(), None).await;
        let result = account.encode_calls(&dummy_call);
        // 0x2 is the Calldata len.
        assert!(*result.get(3).unwrap() == FieldElement::from_hex_be("0x2").unwrap());
    }
}

fn convert_policy(policies: Vec<JsPolicy>) -> Vec<String> {
    policies.into_iter().map(|p| format!("{},{}", p.target, p.method)).collect::<Vec<String>>()
}

fn get_policies_from_abi_entry(
    policies: &mut Vec<Policy>,
    contract_address: FieldElement,
    entries: &[AbiEntry],
) {
    for entry in entries {
        match entry {
            AbiEntry::Function(f) => {
                let selector = f.name.to_string();
                let policy = Policy { target: contract_address, method: selector };

                info!(policy = ?policy, "Adding policy");
                policies.push(policy);
            }

            AbiEntry::Interface(i) => {
                get_policies_from_abi_entry(policies, contract_address, &i.items)
            }

            _ => {}
        }
    }
}

fn get_policies(manifest: DeploymentManifest, root_dir: &Path) -> Vec<Policy> {
    let mut policies: Vec<Policy> = Vec::new();
    let root_dir_ut8: Utf8PathBuf = root_dir.to_path_buf().try_into().unwrap();

    // contracts
    for contract in manifest.contracts {
        let abis = contract.inner.abi.unwrap().load_abi_string(&root_dir_ut8).unwrap();
        let abis = serde_json::from_str::<Vec<AbiEntry>>(&abis).unwrap();
        let contract_address = contract.inner.address.unwrap();
        get_policies_from_abi_entry(&mut policies, contract_address, &abis);
    }

    // world contract
    let abis = manifest.world.inner.abi.unwrap().load_abi_string(&root_dir_ut8).unwrap();
    let abis = serde_json::from_str::<Vec<AbiEntry>>(&abis).unwrap();
    let contract_address = manifest.world.inner.address.unwrap();
    get_policies_from_abi_entry(&mut policies, contract_address, &abis);

    policies
}
