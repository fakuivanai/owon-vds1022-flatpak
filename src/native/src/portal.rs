//! One session-bus connection owns portal requests and the resulting USB grants.
//! Subscribe before sending requests, as required by the portal request protocol:
//! https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.Request.html
//! USB FD acquisition follows:
//! https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.Usb.html

use futures_util::StreamExt;
use serde::{Serialize, de::DeserializeOwned};
use std::{
    collections::HashMap,
    error::Error,
    fmt,
    os::fd::OwnedFd,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::runtime::Runtime;
use zbus::zvariant::{DynamicType, Fd, OwnedObjectPath, OwnedValue, Type};
use zbus::{AsyncDrop, Connection, MatchRule, Message, MessageStream, Proxy};

const PORTAL_NAME: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const REQUEST_INTERFACE: &str = "org.freedesktop.portal.Request";
const USB_INTERFACE: &str = "org.freedesktop.portal.Usb";
const METHOD_TIMEOUT: Duration = Duration::from_secs(10);
static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);

pub type Results = HashMap<String, OwnedValue>;

#[derive(Debug, Eq, PartialEq)]
pub enum PortalError {
    Cancelled,
    Failed(String),
}

impl fmt::Display for PortalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => f.write_str("The portal request was cancelled"),
            Self::Failed(message) => f.write_str(message),
        }
    }
}

impl Error for PortalError {}

impl From<zbus::Error> for PortalError {
    fn from(error: zbus::Error) -> Self {
        Self::Failed(error.to_string())
    }
}

#[derive(Debug)]
pub struct PortalResponse {
    pub path: OwnedObjectPath,
    pub results: Results,
}

#[derive(Clone, Debug)]
pub struct UsbDevice {
    pub id: String,
    pub readable: bool,
    pub writable: bool,
    pub vendor_id: Option<String>,
    pub product_id: Option<String>,
}

impl UsbDevice {
    fn from_information(id: String, mut information: Results) -> Self {
        let readable = information
            .remove("readable")
            .and_then(|value| bool::try_from(value).ok())
            .unwrap_or(false);
        let writable = information
            .remove("writable")
            .and_then(|value| bool::try_from(value).ok())
            .unwrap_or(false);
        let mut properties = information
            .remove("properties")
            .and_then(|value| Results::try_from(value).ok())
            .unwrap_or_default();
        Self {
            id,
            readable,
            writable,
            vendor_id: properties
                .remove("ID_VENDOR_ID")
                .and_then(|value| String::try_from(value).ok()),
            product_id: properties
                .remove("ID_MODEL_ID")
                .and_then(|value| String::try_from(value).ok()),
        }
    }
}

#[derive(Clone)]
pub struct Portal {
    // Drop the connection before its runtime when the last Portal owner goes away.
    connection: Connection,
    runtime: Arc<Runtime>,
}

impl Portal {
    pub fn connect() -> Result<Self, PortalError> {
        let runtime = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .build()
                .map_err(|error| PortalError::Failed(error.to_string()))?,
        );
        let connection = runtime.block_on(Connection::session())?;
        Self::with_connection(connection, runtime)
    }

    /// Inject a private-bus connection and its running executor for tests.
    pub fn with_connection(
        connection: Connection,
        runtime: Arc<Runtime>,
    ) -> Result<Self, PortalError> {
        if !connection.is_bus() || connection.unique_name().is_none() || connection.is_closed() {
            return Err(PortalError::Failed(
                "The session bus connection is unavailable".into(),
            ));
        }
        Ok(Self {
            connection,
            runtime,
        })
    }

    pub fn token(&self) -> String {
        // The object path also contains the bus connection's unique name.
        let sequence = NEXT_TOKEN
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .expect("Portal request token sequence exhausted");
        format!("vds_{sequence}")
    }

    fn request_path(&self, token: &str) -> Result<OwnedObjectPath, PortalError> {
        if token.is_empty()
            || !token
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            return Err(PortalError::Failed("Invalid portal request token".into()));
        }
        let name = self.connection.unique_name().ok_or_else(|| {
            PortalError::Failed("The session bus has no unique connection name".into())
        })?;
        let sender = name
            .as_str()
            .strip_prefix(':')
            .ok_or_else(|| PortalError::Failed("Invalid session bus connection name".into()))?
            .replace('.', "_");
        OwnedObjectPath::try_from(format!("{PORTAL_PATH}/request/{sender}/{token}"))
            .map_err(|error| PortalError::Failed(error.to_string()))
    }

    pub fn request<B>(
        &self,
        interface: &str,
        method: &str,
        body: &B,
        token: &str,
    ) -> Result<PortalResponse, PortalError>
    where
        B: Serialize + DynamicType,
    {
        self.runtime
            .block_on(self.request_async(interface, method, body, token))
    }

    pub async fn request_async<B>(
        &self,
        interface: &str,
        method: &str,
        body: &B,
        token: &str,
    ) -> Result<PortalResponse, PortalError>
    where
        B: Serialize + DynamicType,
    {
        if self.connection.is_closed() {
            return Err(PortalError::Failed(
                "The desktop portal connection has closed".into(),
            ));
        }
        let expected = self.request_path(token)?;
        let proxy = Proxy::new(
            &self.connection,
            PORTAL_NAME,
            expected.as_str(),
            REQUEST_INTERFACE,
        )
        .await?;
        // Proxy signal streams track the name's unique owner, rejecting signals
        // sent directly by another process even when it knows the request path.
        let mut responses = proxy.receive_signal("Response").await?;
        let owner_rule = MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .sender("org.freedesktop.DBus")?
            .path("/org/freedesktop/DBus")?
            .interface("org.freedesktop.DBus")?
            .member("NameOwnerChanged")?
            .add_arg(PORTAL_NAME)?
            .build();
        let mut owners =
            match MessageStream::for_match_rule(owner_rule, &self.connection, Some(8)).await {
                Ok(stream) => stream,
                Err(error) => {
                    responses.async_drop().await;
                    return Err(error.into());
                }
            };
        let result = async {
            let call = self.call_message(interface, method, body);
            tokio::pin!(call);
            let mut early_response = None;
            let reply = loop {
                tokio::select! {
                    _ = self.connection.closed() => return Err(connection_closed()),
                    changed = owners.next() => check_owner_change(changed)?,
                    response = responses.next() => {
                        let response = response.ok_or_else(connection_closed)?;
                        if early_response.is_none() {
                            // Validate the body now, but still require a valid method reply.
                            decode_response(&response)?;
                            early_response = Some(response);
                        }
                    }
                    reply = &mut call => break reply?,
                }
            };
            let returned: OwnedObjectPath = reply.body().deserialize()
                .map_err(|error| PortalError::Failed(format!("Malformed portal request reply: {error}")))?;
            if returned != expected {
                return Err(PortalError::Failed(format!("Portal returned an unexpected request path: {returned}")));
            }
            let reply_sender = reply.header().sender().map(|sender| sender.to_owned())
                .ok_or_else(|| PortalError::Failed("Portal method reply has no sender".into()))?;
            let response = match early_response {
                Some(response) => response,
                None => loop {
                    tokio::select! {
                        _ = self.connection.closed() => return Err(connection_closed()),
                        changed = owners.next() => check_owner_change(changed)?,
                        response = responses.next() => break response.ok_or_else(connection_closed)?,
                    }
                },
            };
            if response.header().sender() != Some(&reply_sender) {
                return Err(PortalError::Failed("Portal response came from an unexpected sender".into()));
            }
            let (code, results) = decode_response(&response)?;
            match code {
                0 => Ok(PortalResponse { path: returned, results }),
                1 => Err(PortalError::Cancelled),
                _ => Err(PortalError::Failed("The portal request failed".into())),
            }
        }.await;
        // Remove both subscriptions before returning, including failed requests.
        responses.async_drop().await;
        owners.async_drop().await;
        result
    }

    async fn call_message<B>(
        &self,
        interface: &str,
        method: &str,
        body: &B,
    ) -> Result<Message, PortalError>
    where
        B: Serialize + DynamicType,
    {
        tokio::time::timeout(
            METHOD_TIMEOUT,
            self.connection.call_method(
                Some(PORTAL_NAME),
                PORTAL_PATH,
                Some(interface),
                method,
                body,
            ),
        )
        .await
        .map_err(|_| PortalError::Failed(format!("Portal method {method} timed out")))?
        .map_err(PortalError::from)
    }

    async fn call_async<B, R>(
        &self,
        interface: &str,
        method: &str,
        body: &B,
    ) -> Result<R, PortalError>
    where
        B: Serialize + DynamicType,
        R: DeserializeOwned + Type,
    {
        self.call_message(interface, method, body)
            .await?
            .body()
            .deserialize()
            .map_err(PortalError::from)
    }

    pub fn enumerate_usb(&self) -> Result<Vec<UsbDevice>, PortalError> {
        let devices: Vec<(String, Results)> = self.runtime.block_on(self.call_async(
            USB_INTERFACE,
            "EnumerateDevices",
            &(Results::new(),),
        ))?;
        Ok(devices
            .into_iter()
            .map(|(id, information)| UsbDevice::from_information(id, information))
            .collect())
    }

    pub fn acquire_usb(&self, id: &str) -> Result<OwnedFd, PortalError> {
        self.runtime.block_on(async {
            let token = self.token();
            let access = Results::from([("writable".into(), true.into())]);
            let options = Results::from([(
                "handle_token".into(),
                zbus::zvariant::Str::from(token.clone()).into(),
            )]);
            let request = self
                .request_async(
                    USB_INTERFACE,
                    "AcquireDevices",
                    &("", vec![(id, access)], options),
                    &token,
                )
                .await?;
            let result = self.finish_usb(id, &request.path).await;
            if result.is_err() {
                // The FD is dropped before releasing any partial acquisition.
                let _ = self.release_usb_async(id).await;
            }
            result
        })
    }

    async fn finish_usb(&self, id: &str, path: &OwnedObjectPath) -> Result<OwnedFd, PortalError> {
        let mut acquired = None;
        loop {
            let (results, finished): (Vec<(String, Results)>, bool) = self
                .call_async(
                    USB_INTERFACE,
                    "FinishAcquireDevices",
                    &(path, Results::new()),
                )
                .await?;
            if results.iter().any(|(candidate, _)| candidate != id) {
                // Drop unsolicited descriptors. Only the requested ID belongs to
                // this acquisition; releasing another ID could revoke an active grant.
                drop(results);
                drop(acquired);
                return Err(PortalError::Failed(
                    "The USB portal returned an unexpected device".into(),
                ));
            }
            for (candidate, mut information) in results {
                debug_assert_eq!(candidate, id);
                let success = information
                    .remove("success")
                    .and_then(|value| bool::try_from(value).ok())
                    .unwrap_or(false);
                if !success {
                    let error = information
                        .remove("error")
                        .and_then(|value| String::try_from(value).ok())
                        .unwrap_or_else(|| {
                            "The USB portal did not grant access to the scope".into()
                        });
                    return Err(PortalError::Failed(error));
                }
                if acquired.is_some() {
                    return Err(PortalError::Failed(
                        "The USB portal returned duplicate device results".into(),
                    ));
                }
                let fd = information.remove("fd").ok_or_else(|| {
                    PortalError::Failed("The USB portal returned no file descriptor".into())
                })?;
                let fd = Fd::try_from(fd)
                    .and_then(OwnedFd::try_from)
                    .map_err(|error| {
                        PortalError::Failed(format!("Invalid USB portal file descriptor: {error}"))
                    })?;
                acquired = Some(fd);
            }
            if finished {
                return acquired.ok_or_else(|| {
                    PortalError::Failed(
                        "The USB portal did not return the scope file descriptor".into(),
                    )
                });
            }
        }
    }

    pub fn release_usb(&self, id: &str) -> Result<(), PortalError> {
        self.runtime.block_on(self.release_usb_async(id))
    }

    async fn release_usb_async(&self, id: &str) -> Result<(), PortalError> {
        self.call_async(USB_INTERFACE, "ReleaseDevices", &(vec![id], Results::new()))
            .await
    }
}

fn connection_closed() -> PortalError {
    PortalError::Failed("The desktop portal connection closed before responding".into())
}

fn decode_response(message: &Message) -> Result<(u32, Results), PortalError> {
    message.body().deserialize().map_err(|error| {
        PortalError::Failed(format!("The portal returned a malformed response: {error}"))
    })
}

fn check_owner_change(change: Option<zbus::Result<Message>>) -> Result<(), PortalError> {
    let message = change.ok_or_else(connection_closed)??;
    let (name, previous, current): (String, String, String) = message.body().deserialize()?;
    if name == PORTAL_NAME && (!previous.is_empty() && previous != current) {
        return Err(PortalError::Failed(
            "The desktop portal stopped before responding".into(),
        ));
    }
    Ok(())
}
