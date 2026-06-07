use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::{Arc, LazyLock},
};

use bichon_core::{
    account::{
        entity::ImapConfig,
        migration::{AccountModel, AccountType},
        since::{DateSince, RelativeDate},
    },
    autoconfig::entity::MailServerConfig,
    cache::imap::mailbox::Attribute,
    database::batch_insert_impl,
    error::{code::ErrorCode, BichonError, BichonResult},
    raise_error,
    token::TokenType,
    users::{acl::AccessControl, role::RoleType},
};
use console::style;
use itertools::Itertools;
use memdb::{Durability, MemDb};
use native_db::*;
use native_model::{native_model, Model};
use serde::{Deserialize, Serialize};

pub const DEFAULT_ADMIN_USER_ID: u64 = 100000000000000;

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[native_model(id = 3, version = 1)]
#[native_db]
pub struct CachedMailSettings {
    #[primary_key]
    pub domain: String,
    pub config: MailServerConfig,
    pub created_at: i64,
}

impl From<CachedMailSettings> for bichon_core::autoconfig::CachedMailSettings {
    fn from(value: CachedMailSettings) -> Self {
        Self {
            domain: value.domain,
            config: value.config,
            created_at: value.created_at,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[native_model(id = 4, version = 1)]
#[native_db(primary_key(pk -> String))]
pub struct AccountV1 {
    #[secondary_key(unique)]
    pub id: u64,
    pub imap: Option<ImapConfig>,
    pub enabled: bool,
    pub email: String,
    pub name: Option<String>,
    pub capabilities: Option<Vec<String>>,
    pub date_since: Option<DateSince>,
    pub folder_limit: Option<u32>,
    pub sync_folders: Option<Vec<String>>,
    pub account_type: AccountType,
    pub sync_interval_min: Option<i64>,
    pub known_folders: Option<BTreeSet<String>>,
    pub created_at: i64,
    pub updated_at: i64,
    pub use_proxy: Option<u64>,
}
impl AccountV1 {
    fn pk(&self) -> String {
        format!("{}_{}", self.created_at, self.id)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[native_model(id = 4, version = 2, from = AccountV1)]
#[native_db(primary_key(pk -> String))]
pub struct AccountV2 {
    #[secondary_key(unique)]
    pub id: u64,
    pub imap: Option<ImapConfig>,
    pub enabled: bool,
    pub email: String,
    pub name: Option<String>,
    pub capabilities: Option<Vec<String>>,
    pub date_since: Option<DateSince>,
    pub folder_limit: Option<u32>,
    pub sync_folders: Option<Vec<String>>,
    pub account_type: AccountType,
    pub sync_interval_min: Option<i64>,
    pub known_folders: Option<BTreeSet<String>>,
    pub created_at: i64,
    pub updated_at: i64,
    pub use_proxy: Option<u64>,
    pub use_dangerous: bool,
    pub pgp_key: Option<String>,
}

impl AccountV2 {
    fn pk(&self) -> String {
        format!("{}_{}", self.created_at, self.id)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[native_model(id = 4, version = 3, from = AccountV2)]
#[native_db(primary_key(pk -> String))]
pub struct AccountV3 {
    #[secondary_key(unique)]
    pub id: u64,
    pub imap: Option<ImapConfig>,
    pub enabled: bool,
    pub email: String,
    pub name: Option<String>,
    pub capabilities: Option<Vec<String>>,
    pub date_since: Option<DateSince>,
    pub date_before: Option<RelativeDate>,
    pub folder_limit: Option<u32>,
    pub sync_folders: Option<Vec<String>>,
    pub account_type: AccountType,
    pub sync_interval_min: Option<i64>,
    pub sync_batch_size: Option<u32>,
    pub known_folders: Option<BTreeSet<String>>,
    pub created_at: i64,
    pub updated_at: i64,
    pub created_by: u64, //user id
    pub use_proxy: Option<u64>,
    pub use_dangerous: bool,
    pub pgp_key: Option<String>,
}

impl AccountV3 {
    fn pk(&self) -> String {
        format!("{}_{}", self.created_at, self.id)
    }
}

impl From<AccountV1> for AccountV2 {
    fn from(value: AccountV1) -> Self {
        Self {
            id: value.id,
            imap: value.imap,
            enabled: value.enabled,
            email: value.email,
            name: value.name,
            capabilities: value.capabilities,
            date_since: value.date_since,
            folder_limit: value.folder_limit,
            sync_folders: value.sync_folders,
            account_type: value.account_type,
            sync_interval_min: value.sync_interval_min,
            known_folders: value.known_folders,
            created_at: value.created_at,
            updated_at: value.updated_at,
            use_proxy: value.use_proxy,
            use_dangerous: false,
            pgp_key: None,
        }
    }
}

impl From<AccountV2> for AccountV1 {
    fn from(value: AccountV2) -> Self {
        Self {
            id: value.id,
            imap: value.imap,
            enabled: value.enabled,
            email: value.email,
            name: value.name,
            capabilities: value.capabilities,
            date_since: value.date_since,
            folder_limit: value.folder_limit,
            sync_folders: value.sync_folders,
            account_type: value.account_type,
            sync_interval_min: value.sync_interval_min,
            known_folders: value.known_folders,
            created_at: value.created_at,
            updated_at: value.updated_at,
            use_proxy: value.use_proxy,
        }
    }
}

impl From<AccountV3> for AccountV2 {
    fn from(value: AccountV3) -> Self {
        Self {
            id: value.id,
            imap: value.imap,
            enabled: value.enabled,
            email: value.email,
            name: value.name,
            capabilities: value.capabilities,
            date_since: value.date_since,
            folder_limit: value.folder_limit,
            sync_folders: value.sync_folders,
            account_type: value.account_type,
            sync_interval_min: value.sync_interval_min,
            known_folders: value.known_folders,
            created_at: value.created_at,
            updated_at: value.updated_at,
            use_proxy: value.use_proxy,
            use_dangerous: value.use_dangerous,
            pgp_key: value.pgp_key,
        }
    }
}

impl From<AccountV2> for AccountV3 {
    fn from(value: AccountV2) -> Self {
        Self {
            id: value.id,
            imap: value.imap,
            enabled: value.enabled,
            email: value.email,
            name: value.name,
            capabilities: value.capabilities,
            date_since: value.date_since,
            folder_limit: value.folder_limit,
            sync_folders: value.sync_folders,
            account_type: value.account_type,
            sync_interval_min: value.sync_interval_min,
            known_folders: value.known_folders,
            created_at: value.created_at,
            updated_at: value.updated_at,
            created_by: DEFAULT_ADMIN_USER_ID,
            use_proxy: value.use_proxy,
            use_dangerous: value.use_dangerous,
            pgp_key: value.pgp_key,
            sync_batch_size: None,
            date_before: None,
        }
    }
}

impl From<AccountV3> for AccountModel {
    fn from(value: AccountV3) -> Self {
        Self {
            id: value.id,
            imap: value.imap,
            enabled: value.enabled,
            email: value.email,
            account_name: None,
            login_name: value.name,
            capabilities: value.capabilities,
            date_since: value.date_since,
            date_before: value.date_before,
            download_folders: value.sync_folders,
            idle_mailboxes: None,
            account_type: value.account_type,
            download_interval_min: value.sync_interval_min,
            download_batch_size: value.sync_batch_size,
            max_email_size_bytes: None,
            known_folders: value.known_folders,
            created_at: value.created_at,
            updated_at: value.updated_at,
            created_by: value.created_by,
            use_proxy: value.use_proxy,
            use_dangerous: value.use_dangerous,
            pgp_key: value.pgp_key,
            imap_quota_window: None,
            imap_quota_bytes: None,
            auto_download_new_mailboxes: None,
            download_schedule: None,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[native_model(id = 5, version = 1)]
#[native_db(primary_key(pk -> String))]
pub struct OAuth2 {
    /// A unique identifier for the OAuth2 configuration.
    #[secondary_key(unique)]
    pub id: u64,
    /// A description of what this configuration is used for.
    pub description: Option<String>,
    /// The client ID used for authenticating the application with the OAuth2 provider.
    pub client_id: String,
    /// The client secret used in conjunction with the client ID.
    ///
    /// Users should provide a plaintext secret.
    /// The server will encrypt it using AES-256-GCM and securely store it.
    /// The plaintext secret is never stored, so users must ensure it is valid for OAuth2 authentication.
    pub client_secret: String,
    /// The URL to redirect users to for OAuth2 authorization.
    pub auth_url: String,
    /// The URL to exchange authorization codes for access tokens.
    pub token_url: String,
    /// The URI where the OAuth2 provider will redirect to after authorization.
    pub redirect_uri: String,
    /// The scopes of access that are being requested (e.g., email, profile).
    pub scopes: Option<Vec<String>>,
    /// Any additional parameters to include in the OAuth2 requests (e.g., access_type, prompt).
    pub extra_params: Option<BTreeMap<String, String>>,
    /// Indicates whether this configuration is enabled or disabled.
    pub enabled: bool,
    /// route OAuth through proxy (when direct access is blocked)
    pub use_proxy: Option<u64>,
    /// The timestamp when the configuration was created, in milliseconds since the Unix epoch.
    pub created_at: i64,
    /// The timestamp when the configuration was last updated, in milliseconds since the Unix epoch.
    pub updated_at: i64,
}

impl OAuth2 {
    fn pk(&self) -> String {
        format!("{}_{}", &self.created_at, &self.id)
    }
}

impl From<OAuth2> for bichon_core::oauth2::entity::OAuth2 {
    fn from(value: OAuth2) -> Self {
        Self {
            id: value.id,
            description: value.description,
            client_id: value.client_id,
            client_secret: value.client_secret,
            auth_url: value.auth_url,
            token_url: value.token_url,
            redirect_uri: value.redirect_uri,
            scopes: value.scopes,
            extra_params: value.extra_params,
            enabled: value.enabled,
            use_proxy: value.use_proxy,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[native_model(id = 6, version = 1)]
#[native_db]
pub struct OAuth2PendingEntity {
    /// Unique identifier for the OAuth2 request record
    pub oauth2_id: u64,

    pub account_id: u64,
    /// CSRF protection state parameter used to verify the integrity of the authorization request
    #[primary_key]
    pub state: String,

    /// PKCE code verifier used in the authorization code exchange process to ensure security
    pub code_verifier: String,

    /// Timestamp when the OAuth2 request was created, used to determine request expiration
    pub created_at: i64,
}

impl From<OAuth2PendingEntity> for bichon_core::oauth2::pending::OAuth2PendingEntity {
    fn from(value: OAuth2PendingEntity) -> Self {
        Self {
            oauth2_id: value.oauth2_id,
            account_id: value.account_id,
            state: value.state,
            code_verifier: value.code_verifier,
            created_at: value.created_at,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[native_model(id = 7, version = 1)]
#[native_db]
pub struct OAuth2AccessToken {
    /// The ID of the account associated with this access token.
    #[primary_key]
    pub account_id: u64,
    /// The id of the OAuth2 configuration associated with this access token.
    #[secondary_key]
    pub oauth2_id: u64,
    /// The OAuth2 access token used to authenticate requests to the provider.
    pub access_token: Option<String>,
    /// The OAuth2 refresh token used to obtain new access tokens.
    pub refresh_token: Option<String>,
    /// The timestamp when the token record was created, in milliseconds since the Unix epoch.
    pub created_at: i64,
    /// The timestamp when the token record was last updated, in milliseconds since the Unix epoch.
    pub updated_at: i64,
}

impl From<OAuth2AccessToken> for bichon_core::oauth2::token::OAuth2AccessToken {
    fn from(value: OAuth2AccessToken) -> Self {
        Self {
            account_id: value.account_id,
            oauth2_id: value.oauth2_id,
            access_token: value.access_token,
            refresh_token: value.refresh_token,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[native_model(id = 8, version = 1)]
#[native_db]
pub struct Proxy {
    /// The unique identifier for this proxy configuration.
    #[primary_key]
    pub id: u64,

    /// The proxy URL (e.g., socks5://127.0.0.1:1080) used to route network requests.
    pub url: String,

    /// The creation timestamp of this record, represented as milliseconds since the Unix epoch.
    pub created_at: i64,

    /// The last update timestamp of this record, represented as milliseconds since the Unix epoch.
    pub updated_at: i64,
}

impl From<Proxy> for bichon_core::settings::proxy::Proxy {
    fn from(value: Proxy) -> Self {
        Self {
            id: value.id,
            url: value.url,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[native_model(id = 9, version = 1)]
#[native_db]
pub struct UserRole {
    #[primary_key]
    pub id: u64,
    pub name: String,
    pub description: Option<String>,
    pub permissions: BTreeSet<String>,
    pub is_builtin: bool,
    pub created_at: i64,
    pub role_type: RoleType,
    pub updated_at: i64,
}

impl From<UserRole> for bichon_core::users::role::UserRole {
    fn from(value: UserRole) -> Self {
        Self {
            id: value.id,
            name: value.name,
            description: value.description,
            permissions: value.permissions,
            is_builtin: value.is_builtin,
            created_at: value.created_at,
            role_type: value.role_type,
            updated_at: value.updated_at,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[native_model(id = 10, version = 1)]
#[native_db]
pub struct BichonUser {
    #[primary_key]
    pub id: u64,
    #[secondary_key(unique)]
    pub username: String,
    #[secondary_key(unique)]
    pub email: String,

    pub password: Option<String>,

    /// Scoped Access: Defines per-account permissions.
    /// Example:
    /// { account_id: 1, role_id: role_manager_id } -> Manager on Account 1
    /// { account_id: 2, role_id: role_viewer_id }  -> Viewer on Account 2
    pub account_access_map: BTreeMap<u64, u64>,

    pub description: Option<String>,

    /// System Roles: Permissions that apply to the whole system
    /// (e.g., system settings, creating new users).
    pub global_roles: Vec<u64>,

    pub avatar: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    /// Optional access control settings
    pub acl: Option<AccessControl>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[native_model(id = 10, version = 2, from = BichonUser)]
#[native_db]
pub struct BichonUserV2 {
    #[primary_key]
    pub id: u64,
    #[secondary_key(unique)]
    pub username: String,
    #[secondary_key(unique)]
    pub email: String,

    pub password: Option<String>,

    /// Scoped Access: Defines per-account permissions.
    /// Example:
    /// { account_id: 1, role_id: role_manager_id } -> Manager on Account 1
    /// { account_id: 2, role_id: role_viewer_id }  -> Viewer on Account 2
    pub account_access_map: BTreeMap<u64, u64>,

    pub description: Option<String>,

    /// System Roles: Permissions that apply to the whole system
    /// (e.g., system settings, creating new users).
    pub global_roles: Vec<u64>,

    pub avatar: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    /// Optional access control settings
    pub acl: Option<AccessControl>,

    pub theme: Option<String>,
    pub language: Option<String>,
}

impl From<BichonUserV2> for BichonUser {
    fn from(value: BichonUserV2) -> Self {
        BichonUser {
            id: value.id,
            username: value.username,
            email: value.email,
            password: value.password,
            account_access_map: value.account_access_map,
            description: value.description,
            global_roles: value.global_roles,
            avatar: value.avatar,
            created_at: value.created_at,
            updated_at: value.updated_at,
            acl: value.acl,
        }
    }
}

impl From<BichonUser> for BichonUserV2 {
    fn from(value: BichonUser) -> Self {
        BichonUserV2 {
            id: value.id,
            username: value.username,
            email: value.email,
            password: value.password,
            account_access_map: value.account_access_map,
            description: value.description,
            global_roles: value.global_roles,
            avatar: value.avatar,
            created_at: value.created_at,
            updated_at: value.updated_at,
            acl: value.acl,
            theme: None,
            language: None,
        }
    }
}

impl From<BichonUserV2> for bichon_core::users::BichonUserV2 {
    fn from(value: BichonUserV2) -> Self {
        Self {
            id: value.id,
            username: value.username,
            email: value.email,
            password: value.password,
            account_access_map: value.account_access_map,
            description: value.description,
            global_roles: value.global_roles,
            avatar: value.avatar,
            created_at: value.created_at,
            updated_at: value.updated_at,
            acl: value.acl,
            theme: value.theme,
            language: value.language,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[native_model(id = 11, version = 1)]
#[native_db]
pub struct AccessTokenModel {
    /// The ID of the user who owns this token
    #[secondary_key]
    pub user_id: u64,
    /// The unique token string used for authentication
    #[primary_key]
    pub token: String,
    /// An optional name of the token.
    pub name: Option<String>,
    /// Token type: WebUI or API
    pub token_type: TokenType,
    /// The timestamp (in milliseconds since epoch) when the token was created.
    pub created_at: i64,
    /// The timestamp (in milliseconds since epoch) when the token was last updated.
    pub updated_at: i64,
    /// The timestamp (in milliseconds since epoch) when the token expires.
    /// None means the token does not expire (this applies only to API tokens).
    pub expire_at: Option<i64>,
    /// The timestamp (in milliseconds since epoch) when the token was last used.
    pub last_access_at: i64,
}

impl From<AccessTokenModel> for bichon_core::token::AccessTokenModel {
    fn from(value: AccessTokenModel) -> Self {
        Self {
            user_id: value.user_id,
            token: value.token,
            name: value.name,
            token_type: value.token_type,
            created_at: value.created_at,
            updated_at: value.updated_at,
            expire_at: value.expire_at,
            last_access_at: value.last_access_at,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[native_model(id = 1, version = 1)]
#[native_db]
pub struct MailBox {
    /// The unique identifier for the mailbox
    #[primary_key]
    pub id: u64,
    /// The ID of the account associated with the mailbox
    #[secondary_key]
    pub account_id: u64,
    /// The unique, decoded, human-readable name of the mailbox (e.g., "INBOX", "Sent Items").
    /// This is the decoded name as presented to users, derived from the IMAP server's mailbox name
    /// (e.g., after decoding UTF-7 or other encodings per RFC 3501).
    pub name: String,
    /// Optional delimiter used to separate mailbox names in a hierarchy (e.g., "/" or ".").
    /// Used in IMAP to structure nested mailboxes (e.g., "INBOX/Archive").
    pub delimiter: Option<String>,
    /// List of attributes associated with the mailbox (e.g., `\NoSelect`, `\Deleted`).
    /// These indicate special properties, such as whether the mailbox can hold messages.
    pub attributes: Vec<Attribute>,
    /// The number of messages that currently exist in the mailbox.
    pub exists: u32,
    /// Optional number of unseen messages in the mailbox (i.e., messages without the `\Seen` flag).
    pub unseen: Option<u32>,
    /// The next unique identifier (UID) that will be assigned to a new message in the mailbox.
    /// If `None`, the IMAP server has not provided this information.
    pub uid_next: Option<u32>,
    /// The validity identifier for UIDs in this mailbox, used to ensure UID consistency across sessions.
    /// If `None`, the IMAP server has not provided this information.
    pub uid_validity: Option<u32>,
}

impl From<MailBox> for bichon_core::cache::imap::mailbox::MailBox {
    fn from(value: MailBox) -> Self {
        Self {
            id: value.id,
            account_id: value.account_id,
            name: value.name,
            delimiter: value.delimiter,
            attributes: value.attributes,
            exists: value.exists,
            unseen: value.unseen,
            uid_next: value.uid_next,
            uid_validity: value.uid_validity,
            highest_uid: None,
        }
    }
}

pub static META_MODELS: LazyLock<Models> = LazyLock::new(|| {
    let mut adapter = ModelsAdapter::new();
    adapter.register_metadata_models();
    adapter.models
});

pub static MAILBOX_MODELS: LazyLock<Models> = LazyLock::new(|| {
    let mut adapter = ModelsAdapter::new();
    adapter.register_model::<MailBox>();
    adapter.models
});

pub struct ModelsAdapter {
    pub models: Models,
}

impl ModelsAdapter {
    pub fn new() -> Self {
        ModelsAdapter {
            models: Models::new(),
        }
    }

    pub fn register_model<T: ToInput>(&mut self) {
        self.models.define::<T>().expect("failed to define model ");
    }

    pub fn register_metadata_models(&mut self) {
        self.register_model::<CachedMailSettings>();
        self.register_model::<AccountV1>();
        self.register_model::<AccountV2>();
        self.register_model::<AccountV3>();
        self.register_model::<OAuth2>();
        self.register_model::<OAuth2PendingEntity>();
        self.register_model::<OAuth2AccessToken>();
        self.register_model::<Proxy>();
        self.register_model::<UserRole>();
        self.register_model::<BichonUser>();
        self.register_model::<BichonUserV2>();
        self.register_model::<AccessTokenModel>();
    }
}

fn init_meta_database(root_path: &PathBuf) -> BichonResult<Arc<Database<'static>>> {
    let mut database = Builder::new()
        .set_cache_size(134217728)
        .create(&META_MODELS, root_path.join("meta.db"))
        .map_err(handle_database_error)?;

    let rw = database
        .rw_transaction()
        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
    rw.migrate::<AccountV3>()
        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
    rw.migrate::<BichonUserV2>()
        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
    rw.commit()
        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

    database
        .compact()
        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
    Ok(Arc::new(database))
}

fn init_evenlope_database(root_path: &PathBuf) -> BichonResult<Arc<Database<'static>>> {
    let mut database = Builder::new()
        .set_cache_size(1073741824)
        .create(&MAILBOX_MODELS, root_path.join("mailbox.db"))
        .map_err(handle_database_error)?;

    let rw = database
        .rw_transaction()
        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
    rw.commit()
        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

    database
        .compact()
        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

    Ok(Arc::new(database))
}

fn handle_database_error(error: native_db::db_type::Error) -> BichonError {
    raise_error!(
        format!("Failed to create database: {:?}", error),
        ErrorCode::InternalError
    )
}

pub fn list_all_impl<T: ToInput + Clone + Send + 'static>(
    database: &Arc<Database<'static>>,
) -> BichonResult<Vec<T>> {
    let r_transaction = database
        .r_transaction()
        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
    let entities: Vec<T> = r_transaction
        .scan()
        .primary()
        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
        .all()
        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
        .try_collect()
        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
    Ok(entities)
}

pub fn migrate_metadata(root_path: &PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    // Pre-flight: verify old metadata databases exist
    let meta_db_path = root_path.join("meta.db");
    if !meta_db_path.exists() {
        return Err(format!(
            "Legacy metadata database not found at '{}'. \
             Make sure the root directory points to a valid v0.3.7 installation.",
            meta_db_path.display()
        )
        .into());
    }
    let mailbox_db_path = root_path.join("mailbox.db");
    if !mailbox_db_path.exists() {
        return Err(format!(
            "Legacy mailbox database not found at '{}'. \
             Make sure the root directory points to a valid v0.3.7 installation.",
            mailbox_db_path.display()
        )
        .into());
    }

    // Initialize legacy database connections
    let meta_db = init_meta_database(root_path)
        .map_err(|e| format!("Failed to initialize legacy metadata database: {}", e))?;
    let envelope_db = init_evenlope_database(root_path)
        .map_err(|e| format!("Failed to initialize legacy envelope database: {}", e))?;

    // Prepare new database directory
    let db_path = root_path.join("memdb");
    if !db_path.exists() {
        std::fs::create_dir_all(&db_path)?;
    }

    // Open new database (disable full durability for faster bulk writes)
    let db = MemDb::open_with(&db_path, Durability::Off)
        .map_err(|e| format!("Failed to open new memdb database: {}", e))?;

    println!(
        "{}",
        style("Step 1: Migrating Metadata Entities...")
            .bold()
            .cyan()
    );

    // Migration helper macro to reduce boilerplate
    macro_rules! migrate_collection {
        ($name:expr, $old_type:ty, $new_type:ty, $source_db:expr) => {
            print!("  > {:<25} ", $name);
            let items = list_all_impl::<$old_type>($source_db)?;
            let count = items.len();
            let converted: Vec<$new_type> = items.into_iter().map(|a| a.into()).collect();
            batch_insert_impl(&db, converted)?;
            println!("{} ({} items)", style("done").green(), count);
        };
    }

    // --- Migrate each entity type ---

    migrate_collection!(
        "Mail Settings",
        CachedMailSettings,
        bichon_core::autoconfig::CachedMailSettings,
        &meta_db
    );

    migrate_collection!("Accounts", AccountV3, AccountModel, &meta_db);

    migrate_collection!(
        "OAuth2 Entities",
        OAuth2,
        bichon_core::oauth2::entity::OAuth2,
        &meta_db
    );

    migrate_collection!(
        "OAuth2 Pending",
        OAuth2PendingEntity,
        bichon_core::oauth2::pending::OAuth2PendingEntity,
        &meta_db
    );

    migrate_collection!(
        "OAuth2 Access Tokens",
        OAuth2AccessToken,
        bichon_core::oauth2::token::OAuth2AccessToken,
        &meta_db
    );

    migrate_collection!(
        "Proxy Settings",
        Proxy,
        bichon_core::settings::proxy::Proxy,
        &meta_db
    );

    migrate_collection!(
        "User Roles",
        UserRole,
        bichon_core::users::role::UserRole,
        &meta_db
    );

    migrate_collection!(
        "Users",
        BichonUserV2,
        bichon_core::users::BichonUserV2,
        &meta_db
    );

    migrate_collection!(
        "Access Tokens",
        AccessTokenModel,
        bichon_core::token::AccessTokenModel,
        &meta_db
    );

    // Mailboxes (from envelope_db)
    migrate_collection!(
        "Mailboxes",
        MailBox,
        bichon_core::cache::imap::mailbox::MailBox,
        &envelope_db
    );

    // Persist and finish
    db.snapshot()
        .map_err(|e| format!("Snapshot save failed: {}", e))?;
    println!(
        "{}",
        style("Metadata migration completed successfully.")
            .green()
            .bold()
    );

    Ok(())
}
