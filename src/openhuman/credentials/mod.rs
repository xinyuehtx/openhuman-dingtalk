//! Credential management for app session and provider auth profiles.

pub mod bus;
pub mod cli;
mod core;
pub mod ops;
pub mod profiles;
pub mod responses;
mod schemas;
pub mod session_support;

/// Default user identity for custom-LLM / offline mode when no backend
/// session exists. Used as a stable `user_id` so agent, memory, and
/// socket subsystems have a consistent identity without requiring login.
pub const CUSTOM_LLM_LOCAL_USER_ID: &str = "local-user";

pub use crate::api::rest::{
    decrypt_handoff_blob, user_id_from_auth_me_payload, user_id_from_profile_payload,
    BackendOAuthClient, ConnectResponse, IntegrationSummary, IntegrationTokensHandoff,
};
pub use core::*;
pub use ops as rpc;
pub use ops::*;
// Direct-mode (BYO Composio API key) credential helpers.
pub use ops::{
    clear_composio_api_key, get_composio_api_key, store_composio_api_key, COMPOSIO_DIRECT_PROVIDER,
};
pub use schemas::{
    all_controller_schemas as all_credentials_controller_schemas,
    all_registered_controllers as all_credentials_registered_controllers,
};
