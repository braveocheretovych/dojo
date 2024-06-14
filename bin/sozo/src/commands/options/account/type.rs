use async_trait::async_trait;
use starknet::accounts::single_owner::SignError;
use std::sync::Arc;

use starknet::accounts::{Account, Call, RawExecution, RawLegacyDeclaration};
use starknet::accounts::{ConnectedAccount, ExecutionEncoder, SingleOwnerAccount};
use starknet::accounts::{Declaration, Execution, LegacyDeclaration, RawDeclaration};
use starknet::core::types::contract::legacy::LegacyContractClass;
use starknet::core::types::{FieldElement, FlattenedSierraClass};
use starknet::providers::Provider;
use starknet::signers::{LocalWallet, SigningKey};

#[cfg(feature = "controller")]
use account_sdk::account::session::SessionAccount;

#[derive(Debug, thiserror::Error)]
pub enum SozoAccountSignError {
    #[cfg(feature = "controller")]
    #[error("Controller error: {0}")]
    Controller(#[from] account_sdk::signers::SignError),
    #[error("Standard error: {0}")]
    Standard(#[from] SignError<starknet::signers::local_wallet::SignError>),
}

/// To unify the account types, we define a SozoAccount enum that can be either a standard account or a controller account.
#[must_use]
#[non_exhaustive]
pub enum SozoAccount<P>
where
    P: Provider + Send,
{
    Standard(SingleOwnerAccount<P, LocalWallet>),
    #[cfg(feature = "controller")]
    Controller(SessionAccount<P, SigningKey, SigningKey>),
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl<P> Account for SozoAccount<P>
where
    P: Provider + Send + Sync,
{
    type SignError = SozoAccountSignError;

    fn address(&self) -> FieldElement {
        match self {
            Self::Standard(account) => account.address(),
            #[cfg(feature = "controller")]
            Self::Controller(account) => account.address(),
        }
    }

    fn chain_id(&self) -> FieldElement {
        match self {
            Self::Standard(account) => account.chain_id(),
            #[cfg(feature = "controller")]
            Self::Controller(account) => account.chain_id(),
        }
    }

    fn declare(
        &self,
        contract_class: Arc<FlattenedSierraClass>,
        compiled_class_hash: FieldElement,
    ) -> Declaration<Self> {
        Declaration::new(contract_class, compiled_class_hash, self)
    }

    fn declare_legacy(&self, contract_class: Arc<LegacyContractClass>) -> LegacyDeclaration<Self> {
        LegacyDeclaration::new(contract_class, self)
    }

    fn execute(&self, calls: Vec<Call>) -> Execution<Self> {
        Execution::new(calls, self)
    }

    async fn sign_execution(
        &self,
        execution: &RawExecution,
        query_only: bool,
    ) -> Result<Vec<FieldElement>, Self::SignError> {
        let result = match self {
            Self::Standard(account) => account.sign_execution(execution, query_only).await?,
            #[cfg(feature = "controller")]
            Self::Controller(account) => account.sign_execution(execution, query_only).await?,
        };
        Ok(result)
    }

    async fn sign_declaration(
        &self,
        declaration: &RawDeclaration,
        query_only: bool,
    ) -> Result<Vec<FieldElement>, Self::SignError> {
        let result = match self {
            Self::Standard(account) => account.sign_declaration(declaration, query_only).await?,
            #[cfg(feature = "controller")]
            Self::Controller(account) => account.sign_declaration(declaration, query_only).await?,
        };
        Ok(result)
    }

    async fn sign_legacy_declaration(
        &self,
        declaration: &RawLegacyDeclaration,
        query_only: bool,
    ) -> Result<Vec<FieldElement>, Self::SignError> {
        match self {
            Self::Standard(account) => {
                let result = account.sign_legacy_declaration(declaration, query_only).await?;
                Ok(result)
            }
            #[cfg(feature = "controller")]
            Self::Controller(account) => {
                let result = account.sign_legacy_declaration(declaration, query_only).await?;
                Ok(result)
            }
        }
    }
}

impl<P> ExecutionEncoder for SozoAccount<P>
where
    P: Provider + Send,
{
    fn encode_calls(&self, calls: &[Call]) -> Vec<FieldElement> {
        match self {
            Self::Standard(account) => account.encode_calls(calls),
            #[cfg(feature = "controller")]
            Self::Controller(account) => account.encode_calls(calls),
        }
    }
}

impl<P> ConnectedAccount for SozoAccount<P>
where
    P: Provider + Send + Sync,
{
    type Provider = P;

    fn provider(&self) -> &Self::Provider {
        match self {
            Self::Standard(account) => account.provider(),
            #[cfg(feature = "controller")]
            Self::Controller(account) => account.provider(),
        }
    }
}
