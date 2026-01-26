use std::{fmt, io, path::StripPrefixError, sync::mpsc::SendError};

#[cfg(feature = "service")]
use std::{borrow::Cow, error::Error as StdError};

use http::status::StatusCode;
use pulldown_cmark_to_cmark::Error as CmarkToCmarkError;
use regex::Error as RegexError;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::mpsc::error::SendError as TokioSendError;
use url::ParseError as UrlParseError;

// #[cfg(feature = "tauri")]
// use tauri_plugin_store::Error as StoreError;

// #[cfg(feature = "tauri")]
// use tauri::Error as TauriError;

#[cfg(feature = "service")]
use notify::{Error as NotifyError, ErrorKind as NotifyErrorKind};

#[cfg(feature = "service")]
use sqlx::{
    error::{DatabaseError, ErrorKind as DatabaseErrorKind},
    Error as SqlxError,
};

use serde_json::Error as JsonError;

#[cfg(feature = "wasm")]
use serde_wasm_bindgen::Error as WasmError;

use crate::event::{BeliefEvent, Event};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Error, uniffi::Error)]
#[uniffi(flat_error)]
pub enum BuildonomyError {
    #[error("Cache/Database error: {0}")]
    Cache(String),
    #[error("Buildonomy codec software error: {0}")]
    Codec(String),
    #[error("Invalid Command: {0}")]
    Command(String),
    #[error("Custom error: {0}")]
    Custom(String),
    #[error("File System error: {0}")]
    Io(String),
    #[error("Item Not Found: {0}")]
    NotFound(String),
    #[error("Participant Dialog Cancellation Event")]
    OperationCancelled,
    #[error("Page Not Found")]
    PageNotFound,
    #[error("You do not have permission to access this resource")]
    PermissionDenied,
    #[error("(De)Serialization error: {0}")]
    Serialization(String),
    #[error("Service API error: {0}")]
    Service(String),
    #[error("Unresolved network reference '{network_ref}' in {key_type}://{network_ref}/{value}. Use resolve_network() to resolve via BeliefSource.")]
    UnresolvedNetwork {
        network_ref: String,
        key_type: String,
        value: String,
    },
}

impl BuildonomyError {
    pub fn status_code(&self) -> StatusCode {
        match self {
            BuildonomyError::Cache(_) => StatusCode::INTERNAL_SERVER_ERROR,
            BuildonomyError::Codec(_) => StatusCode::INTERNAL_SERVER_ERROR,
            BuildonomyError::Command(_) => StatusCode::BAD_REQUEST,
            BuildonomyError::Custom(_) => StatusCode::INTERNAL_SERVER_ERROR,
            BuildonomyError::Io(_) => StatusCode::INTERNAL_SERVER_ERROR,
            BuildonomyError::NotFound(_) => StatusCode::NOT_FOUND,
            BuildonomyError::OperationCancelled => StatusCode::NO_CONTENT,
            BuildonomyError::PageNotFound => StatusCode::NOT_FOUND,
            BuildonomyError::PermissionDenied => StatusCode::FORBIDDEN,
            BuildonomyError::Serialization(_) => StatusCode::INTERNAL_SERVER_ERROR,
            BuildonomyError::Service(_) => StatusCode::INTERNAL_SERVER_ERROR,
            BuildonomyError::UnresolvedNetwork { .. } => StatusCode::BAD_REQUEST,
        }
    }
}

impl From<StripPrefixError> for BuildonomyError {
    fn from(src: StripPrefixError) -> BuildonomyError {
        BuildonomyError::NotFound(format!("Strip prefix failed for path. Error: {src}"))
    }
}

impl From<toml::de::Error> for BuildonomyError {
    fn from(src: toml::de::Error) -> BuildonomyError {
        BuildonomyError::Serialization(format!("Toml deserialization error: {src}"))
    }
}

impl From<toml::ser::Error> for BuildonomyError {
    fn from(src: toml::ser::Error) -> BuildonomyError {
        BuildonomyError::Serialization(format!("Toml serialization error: {src}"))
    }
}

impl From<JsonError> for BuildonomyError {
    fn from(src: JsonError) -> BuildonomyError {
        BuildonomyError::Serialization(format!("JSON (de)serialization error: {src}"))
    }
}

impl From<uuid::Error> for BuildonomyError {
    fn from(src: uuid::Error) -> BuildonomyError {
        BuildonomyError::Serialization(format!("UUID conversion failed: {src}"))
    }
}

impl From<UrlParseError> for BuildonomyError {
    fn from(src: UrlParseError) -> BuildonomyError {
        BuildonomyError::Serialization(format!("Invalid URL: {src}"))
    }
}

impl From<io::Error> for BuildonomyError {
    fn from(x: io::Error) -> Self {
        match x.kind() {
            io::ErrorKind::NotFound => BuildonomyError::NotFound(format!("{x}")),
            io::ErrorKind::PermissionDenied => BuildonomyError::PermissionDenied,
            _ => BuildonomyError::Io(format!("IOError: {}", x.kind())),
        }
    }
}

impl From<fmt::Error> for BuildonomyError {
    fn from(x: fmt::Error) -> Self {
        BuildonomyError::Codec(format!("{x}"))
    }
}

impl From<CmarkToCmarkError> for BuildonomyError {
    fn from(x: CmarkToCmarkError) -> Self {
        BuildonomyError::Codec(format!("{x}"))
    }
}

impl From<RegexError> for BuildonomyError {
    fn from(x: RegexError) -> Self {
        BuildonomyError::Serialization(format!("Regex parse failed: {x}"))
    }
}

impl From<SendError<Event>> for BuildonomyError {
    fn from(x: SendError<Event>) -> Self {
        BuildonomyError::Io(format!(
            "Channel update send Error, could not transmit state update event {:?}",
            x.0
        ))
    }
}

impl From<TokioSendError<BeliefEvent>> for BuildonomyError {
    fn from(x: TokioSendError<BeliefEvent>) -> Self {
        BuildonomyError::Io(format!(
            "Channel update send Error, could not transmit state update event {:?}",
            x.0
        ))
    }
}

#[cfg(feature = "wasm")]
impl From<WasmError> for BuildonomyError {
    fn from(wasm_error: WasmError) -> Self {
        BuildonomyError::Serialization(format!("Serde-wasm-bindgen error: {wasm_error}"))
    }
}

// #[cfg(feature = "tauri")]
// impl From<TauriError> for BuildonomyError {
//     fn from(error: TauriError) -> Self {
//         BuildonomyError::Tauri(format!("[Tauri]: {:?}", error))
//     }
// }

// #[cfg(feature = "tauri")]
// impl From<StoreError> for BuildonomyError {
//     fn from(store_error: StoreError) -> Self {
//         match store_error {
//             StoreError::Serialize(err) | StoreError::Deserialize(err) => {
//                 BuildonomyError::Serialization(format!("{}", err))
//             }
//             StoreError::Json(err) => BuildonomyError::Serialization(format!("{}", err)),
//             StoreError::Io(err) => BuildonomyError::Io(format!("{}", err)),
//             StoreError::SerializeFunctionNotFound(path) => {
//                 BuildonomyError::Tauri(format!("{:?}", path))
//             }
//             StoreError::Tauri(err) => BuildonomyError::Tauri(format!("{}", err)),
//             _ => BuildonomyError::Custom(format!("{:?}", store_error)),
//         }
//     }
// }

#[cfg(feature = "service")]
impl From<NotifyError> for BuildonomyError {
    fn from(notify_error: NotifyError) -> Self {
        match notify_error.kind {
            NotifyErrorKind::Generic(msg) => BuildonomyError::Custom(format!(
                "notify-debouncer: {}, paths: {:?}",
                msg, notify_error.paths
            )),
            NotifyErrorKind::Io(io_error) => BuildonomyError::Custom(format!(
                "notify-debouncer: io error {}, paths: {:?}",
                io_error.kind(),
                notify_error.paths
            )),
            NotifyErrorKind::PathNotFound => BuildonomyError::NotFound(format!(
                "notify-debouncer: path(s) not found: {:?}",
                notify_error.paths
            )),
            NotifyErrorKind::WatchNotFound => BuildonomyError::NotFound(format!(
                "notify-debouncer: watch not found, paths: {:?}",
                notify_error.paths
            )),
            NotifyErrorKind::InvalidConfig(_) => {
                BuildonomyError::Custom("notify-debouncer invalid config".to_string())
            }
            NotifyErrorKind::MaxFilesWatch => {
                BuildonomyError::Custom("notify-debouncer max file watch limit reached".to_string())
            }
        }
    }
}

#[cfg(feature = "service")]
impl From<SqlxError> for BuildonomyError {
    fn from(db_error: SqlxError) -> Self {
        BuildonomyError::Io(format!("database error: {db_error:?}"))
    }
}

#[cfg(feature = "service")]
impl DatabaseError for BuildonomyError {
    fn message(&self) -> &str {
        "Buildonomy FromRow parsing failure"
    }

    fn kind(&self) -> sqlx::error::ErrorKind {
        DatabaseErrorKind::Other
    }

    /// The extended result code.
    #[inline]
    fn code(&self) -> Option<Cow<'_, str>> {
        None
    }

    #[doc(hidden)]
    fn as_error(&self) -> &(dyn StdError + Send + Sync + 'static) {
        self
    }

    #[doc(hidden)]
    fn as_error_mut(&mut self) -> &mut (dyn StdError + Send + Sync + 'static) {
        self
    }

    #[doc(hidden)]
    fn into_error(self: Box<Self>) -> Box<dyn StdError + Send + Sync + 'static> {
        self
    }
}
