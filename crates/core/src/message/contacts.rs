//use poem_openapi::Object;
use serde::{Deserialize, Serialize};

/// One address in the search UI's suggestion list.
///
/// `name` is the sender display name the archive recorded for this address
/// (`from_name`); it is `None` for addresses that only ever appeared as a
/// recipient, because `to`/`cc`/`bcc` are stored as bare addresses.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct Contact {
    pub email: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}


