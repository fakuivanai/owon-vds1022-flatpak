//! USB portal grants and libusb handles have separate lifetimes.
//!
//! Protocol: https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.Usb.html
//! Wrapping contract: https://docs.rs/rusb/0.9.4/rusb/trait.UsbContext.html#method.open_device_with_fd

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::Arc;
use std::time::Duration;

use rusb::UsbContext;

use crate::portal::{PortalError, UsbDevice};

const VENDOR_ID: u16 = 0x5345;
const PRODUCT_ID: u16 = 0x1234;
pub type Metadata = [i32; 8];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Cancelled,
    InvalidArgument(String),
    Usb(String),
    Portal(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => f.write_str("USB access was cancelled"),
            Self::InvalidArgument(message) | Self::Usb(message) | Self::Portal(message) => {
                f.write_str(message)
            }
        }
    }
}
impl std::error::Error for Error {}

impl From<PortalError> for Error {
    fn from(error: PortalError) -> Self {
        match error {
            PortalError::Cancelled => Self::Cancelled,
            PortalError::Failed(message) => Self::Portal(message),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportError {
    NoDevice,
    Failed(String),
}

impl From<TransportError> for Error {
    fn from(error: TransportError) -> Self {
        match error {
            TransportError::NoDevice => Self::Usb("The USB device is disconnected".into()),
            TransportError::Failed(message) => Self::Usb(message),
        }
    }
}

impl From<rusb::Error> for TransportError {
    fn from(error: rusb::Error) -> Self {
        if error == rusb::Error::NoDevice {
            Self::NoDevice
        } else {
            Self::Failed(error.to_string())
        }
    }
}

pub trait PortalBackend {
    fn enumerate(&mut self) -> Result<Vec<UsbDevice>, Error>;
    /// Acquisition failures must release any grant created before the failure.
    fn acquire(&mut self, id: &str) -> Result<OwnedFd, Error>;
    fn release(&mut self, id: &str) -> Result<(), Error>;
}

#[derive(Default)]
pub struct DefaultPortal;

impl PortalBackend for DefaultPortal {
    fn enumerate(&mut self) -> Result<Vec<UsbDevice>, Error> {
        Ok(crate::portal()?.enumerate_usb()?)
    }
    fn acquire(&mut self, id: &str) -> Result<OwnedFd, Error> {
        Ok(crate::portal()?.acquire_usb(id)?)
    }
    fn release(&mut self, id: &str) -> Result<(), Error> {
        Ok(crate::portal()?.release_usb(id)?)
    }
}

/// Owns only the portal descriptor, with no libusb handle or interface claim.
pub struct PortalGrant {
    id: String,
    fd: Arc<OwnedFd>,
}

#[derive(Debug, Clone)]
pub struct EndpointDescription {
    address: u8,
    bulk: bool,
    packet_size: u16,
}

#[derive(Debug, Clone)]
pub struct InterfaceDescription {
    number: u8,
    alternate: u8,
    endpoints: Vec<EndpointDescription>,
}

#[derive(Debug, Clone)]
pub struct DeviceDescription {
    vendor: u16,
    product: u16,
    configuration: u8,
    usb_version: u16,
    interfaces: Vec<InterfaceDescription>,
}

pub trait UsbBackend {
    type Handle;
    fn init(&mut self) -> Result<(), TransportError>;
    /// The caller keeps the grant alive until this handle is dropped.
    fn wrap(&mut self, grant: &PortalGrant) -> Result<Self::Handle, TransportError>;
    fn describe(&mut self, handle: &Self::Handle) -> Result<DeviceDescription, TransportError>;
    fn claim(&mut self, handle: &Self::Handle) -> Result<(), TransportError>;
    fn release_interface(&mut self, handle: &Self::Handle) -> Result<(), TransportError>;
    fn reset(&mut self, handle: &Self::Handle) -> Result<(), TransportError>;
    fn transfer(
        &mut self,
        handle: &Self::Handle,
        endpoint: u8,
        buffer: &mut [u8],
        timeout: Duration,
        read: bool,
    ) -> Result<usize, TransportError>;
    fn shutdown(&mut self);
}

#[derive(Default)]
pub struct LibusbBackend {
    context: Option<rusb::Context>,
}

/// The descriptor stays owned even if cleanup unwinds after removing an active device.
/// Rust drops fields in declaration order: close libusb before the last FD reference.
pub struct LibusbHandle {
    usb: rusb::DeviceHandle<rusb::Context>,
    _fd: Arc<OwnedFd>,
}

impl UsbBackend for LibusbBackend {
    type Handle = LibusbHandle;

    fn init(&mut self) -> Result<(), TransportError> {
        if self.context.is_none() {
            // This option must precede Context::new; with_options applies options too late.
            // https://docs.rs/rusb/0.9.4/rusb/fn.disable_device_discovery.html
            rusb::disable_device_discovery()?;
            self.context = Some(rusb::Context::new()?);
        }
        Ok(())
    }

    fn wrap(&mut self, grant: &PortalGrant) -> Result<Self::Handle, TransportError> {
        let context = self
            .context
            .as_ref()
            .expect("USB context initialized before wrap");
        // SAFETY: only the USB portal creates PortalGrant descriptors. LibusbHandle owns
        // an Arc to this FD and drops its libusb handle before that Arc, including during
        // unwinding. Cleanup drops LibusbHandle before releasing the portal grant.
        // https://docs.rs/rusb/0.9.4/rusb/trait.UsbContext.html#method.open_device_with_fd
        let usb = unsafe { context.open_device_with_fd(grant.fd.as_raw_fd()) }?;
        Ok(LibusbHandle {
            usb,
            _fd: Arc::clone(&grant.fd),
        })
    }

    fn describe(&mut self, handle: &Self::Handle) -> Result<DeviceDescription, TransportError> {
        let device = handle.usb.device();
        let descriptor = device.device_descriptor()?;
        let configuration = device.active_config_descriptor()?;
        let version = descriptor.usb_version();
        let interfaces = configuration
            .interfaces()
            .flat_map(|interface| {
                interface.descriptors().map(|setting| InterfaceDescription {
                    number: setting.interface_number(),
                    alternate: setting.setting_number(),
                    endpoints: setting
                        .endpoint_descriptors()
                        .map(|endpoint| EndpointDescription {
                            address: endpoint.address(),
                            bulk: endpoint.transfer_type() == rusb::TransferType::Bulk,
                            packet_size: endpoint.max_packet_size(),
                        })
                        .collect(),
                })
            })
            .collect();
        Ok(DeviceDescription {
            vendor: descriptor.vendor_id(),
            product: descriptor.product_id(),
            configuration: configuration.number(),
            usb_version: u16::from(version.major()) << 8
                | u16::from(version.minor()) << 4
                | u16::from(version.sub_minor()),
            interfaces,
        })
    }

    fn claim(&mut self, handle: &Self::Handle) -> Result<(), TransportError> {
        match handle.usb.set_auto_detach_kernel_driver(true) {
            Ok(()) | Err(rusb::Error::NotSupported) => {}
            Err(error) => return Err(error.into()),
        }
        handle.usb.claim_interface(0).map_err(Into::into)
    }

    fn release_interface(&mut self, handle: &Self::Handle) -> Result<(), TransportError> {
        handle.usb.release_interface(0).map_err(Into::into)
    }

    fn reset(&mut self, handle: &Self::Handle) -> Result<(), TransportError> {
        handle.usb.reset().map_err(Into::into)
    }

    fn transfer(
        &mut self,
        handle: &Self::Handle,
        endpoint: u8,
        buffer: &mut [u8],
        timeout: Duration,
        read: bool,
    ) -> Result<usize, TransportError> {
        // rusb returns partial progress on timeout, preserving libusb 0.1's contract.
        // It also returns partial progress after interruption.
        // https://docs.rs/rusb/0.9.4/src/rusb/device_handle.rs.html#457-537
        if read {
            handle
                .usb
                .read_bulk(endpoint, buffer, timeout)
                .map_err(Into::into)
        } else {
            handle
                .usb
                .write_bulk(endpoint, buffer, timeout)
                .map_err(Into::into)
        }
    }

    fn shutdown(&mut self) {
        self.context = None;
    }
}

/// Field order is part of the descriptor-wrapping safety contract.
struct ActiveDevice<H> {
    handle: H,
    grant: PortalGrant,
    metadata: Metadata,
}

pub type DefaultController = Controller<DefaultPortal, LibusbBackend>;

pub struct Controller<P: PortalBackend = DefaultPortal, U: UsbBackend = LibusbBackend> {
    portal: P,
    usb: U,
    active: BTreeMap<i64, ActiveDevice<U::Handle>>,
    pending: BTreeMap<String, PortalGrant>,
    next_token: u64,
}

impl DefaultController {
    pub fn new() -> Self {
        Self::with_backends(DefaultPortal, LibusbBackend::default())
    }
}

impl Default for DefaultController {
    fn default() -> Self {
        Self::new()
    }
}

impl<P: PortalBackend, U: UsbBackend> Controller<P, U> {
    pub fn with_backends(portal: P, usb: U) -> Self {
        Self {
            portal,
            usb,
            active: BTreeMap::new(),
            pending: BTreeMap::new(),
            next_token: 1,
        }
    }

    pub fn init(&mut self) -> Result<(), Error> {
        self.usb.init().map_err(Into::into)
    }

    pub fn enumerate(&mut self) -> Result<Vec<String>, Error> {
        let devices = match self.portal.enumerate() {
            Ok(devices) => devices,
            Err(error) => {
                let _ = self.clear_pending();
                return Err(error);
            }
        };
        let ids: Vec<String> = devices
            .into_iter()
            .filter(scope_properties)
            .map(|d| d.id)
            .collect();
        let present: BTreeSet<&str> = ids.iter().map(String::as_str).collect();
        let disappeared: Vec<String> = self
            .pending
            .keys()
            .filter(|id| !present.contains(id.as_str()))
            .cloned()
            .collect();
        let mut first_error = None;
        for id in disappeared {
            if let Err(error) = self.forget(&id) {
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(ids), Err)
    }

    pub fn open(&mut self, id: &str) -> Result<(i64, Metadata), Error> {
        validate_id(id)?;
        if self.active.values().any(|device| device.grant.id == id) {
            return Err(Error::Usb("This scope is already open".into()));
        }
        // Consume before validation: a disappeared or changed probe cannot remain cached.
        let retained = self.pending.remove(id);
        let prepared = self.init().and_then(|()| {
            self.enumerate().and_then(|ids| {
                if ids.iter().any(|candidate| candidate == id) {
                    Ok(())
                } else {
                    Err(Error::Usb(
                        "The selected scope is unavailable through the USB portal".into(),
                    ))
                }
            })
        });
        if let Err(error) = prepared {
            if let Some(grant) = retained {
                let _ = self.dispose_grant(grant);
            }
            return Err(error);
        }
        let grant = match retained {
            Some(grant) => grant,
            None => PortalGrant {
                id: id.to_owned(),
                fd: Arc::new(self.portal.acquire(id)?),
            },
        };
        let handle = match self.usb.wrap(&grant) {
            Ok(handle) => handle,
            Err(error) => {
                let _ = self.dispose_grant(grant);
                return Err(error.into());
            }
        };
        let metadata = self
            .usb
            .describe(&handle)
            .map_err(Error::from)
            .and_then(|description| validate_description(&description));
        let metadata = match metadata {
            Ok(metadata) => metadata,
            Err(error) => {
                drop(handle);
                let _ = self.dispose_grant(grant);
                return Err(error);
            }
        };
        if let Err(error) = self.usb.claim(&handle) {
            drop(handle);
            let _ = self.dispose_grant(grant);
            return Err(error.into());
        }
        if self.next_token > i64::MAX as u64 {
            let _ = self.usb.release_interface(&handle);
            drop(handle);
            let _ = self.dispose_grant(grant);
            return Err(Error::Usb("USB handle sequence exhausted".into()));
        }
        let token = self.next_token as i64;
        self.next_token += 1;
        self.active.insert(
            token,
            ActiveDevice {
                handle,
                grant,
                metadata,
            },
        );
        Ok((token, metadata))
    }

    pub fn close(&mut self, token: i64) -> Result<(), Error> {
        let device = self.active.remove(&token).ok_or_else(invalid_handle)?;
        self.dispose_active(device)
    }

    pub fn close_probe(&mut self, token: i64) -> Result<(), Error> {
        let ActiveDevice { handle, grant, .. } =
            self.active.remove(&token).ok_or_else(invalid_handle)?;
        let released = self.usb.release_interface(&handle);
        drop(handle);
        if let Err(error) = released {
            let cleanup = self.dispose_grant(grant);
            return cleanup.and(Err(error.into()));
        }
        // Duplicate opens are rejected and reopening consumes this map entry first.
        assert!(
            !self.pending.contains_key(&grant.id),
            "duplicate pending USB grant"
        );
        self.pending.insert(grant.id.clone(), grant);
        Ok(())
    }

    pub fn forget(&mut self, id: &str) -> Result<(), Error> {
        validate_id(id)?;
        match self.pending.remove(id) {
            Some(grant) => self.dispose_grant(grant),
            None => Ok(()),
        }
    }

    pub fn reset(&mut self, token: i64) -> Result<(), Error> {
        let device = self.active.remove(&token).ok_or_else(invalid_handle)?;
        let reset = self.usb.reset(&device.handle);
        let cleanup = self.dispose_active(device);
        match reset {
            Ok(()) | Err(TransportError::NoDevice) => cleanup,
            Err(error) => Err(error.into()),
        }
    }

    pub fn transfer(
        &mut self,
        token: i64,
        buffer: &mut [u8],
        timeout_ms: i32,
        read: bool,
    ) -> Result<usize, Error> {
        if buffer.is_empty() || buffer.len() > i32::MAX as usize || timeout_ms <= 0 {
            return Err(Error::InvalidArgument(
                "Invalid USB transfer buffer, size or timeout".into(),
            ));
        }
        let device = self.active.get(&token).ok_or_else(invalid_handle)?;
        let endpoint = device.metadata[if read { 3 } else { 4 }] as u8;
        let result = self.usb.transfer(
            &device.handle,
            endpoint,
            buffer,
            Duration::from_millis(timeout_ms as u64),
            read,
        );
        // Java keeps ownership of its token after transfer failures, including
        // unplugging. Its explicit close/reset or adapter shutdown releases the grant.
        match result {
            Ok(length) if length > buffer.len() => Err(Error::Usb(
                "USB transport returned an invalid transfer length".into(),
            )),
            result => result.map_err(Into::into),
        }
    }

    pub fn shutdown(&mut self) -> Result<(), Error> {
        let active = std::mem::take(&mut self.active);
        let mut first_error = None;
        for (_, device) in active {
            if let Err(error) = self.dispose_active(device) {
                first_error.get_or_insert(error);
            }
        }
        if let Err(error) = self.clear_pending() {
            first_error.get_or_insert(error);
        }
        self.usb.shutdown();
        first_error.map_or(Ok(()), Err)
    }

    fn dispose_active(&mut self, device: ActiveDevice<U::Handle>) -> Result<(), Error> {
        let ActiveDevice { handle, grant, .. } = device;
        let interface = self.usb.release_interface(&handle);
        drop(handle);
        let portal = self.dispose_grant(grant);
        portal?;
        match interface {
            Ok(()) | Err(TransportError::NoDevice) => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    fn dispose_grant(&mut self, grant: PortalGrant) -> Result<(), Error> {
        let PortalGrant { id, fd } = grant;
        drop(fd);
        self.portal.release(&id)
    }

    fn clear_pending(&mut self) -> Result<(), Error> {
        let pending = std::mem::take(&mut self.pending);
        let mut first_error = None;
        for (_, grant) in pending {
            if let Err(error) = self.dispose_grant(grant) {
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }
}

impl<P: PortalBackend, U: UsbBackend> Drop for Controller<P, U> {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

fn invalid_handle() -> Error {
    Error::Usb("The USB device handle is closed or invalid".into())
}

fn validate_id(id: &str) -> Result<(), Error> {
    if id.is_empty() || id.contains('\0') {
        Err(Error::InvalidArgument("Invalid scope ID".into()))
    } else {
        Ok(())
    }
}

fn scope_properties(device: &UsbDevice) -> bool {
    device.readable
        && device.writable
        && device
            .vendor_id
            .as_deref()
            .is_some_and(|id| id.eq_ignore_ascii_case("5345"))
        && device
            .product_id
            .as_deref()
            .is_some_and(|id| id.eq_ignore_ascii_case("1234"))
}

fn validate_description(device: &DeviceDescription) -> Result<Metadata, Error> {
    if device.vendor != VENDOR_ID || device.product != PRODUCT_ID {
        return Err(Error::Usb(
            "The granted USB descriptor is not a supported scope".into(),
        ));
    }
    for setting in &device.interfaces {
        if setting.number != 0 || setting.alternate != 0 {
            continue;
        }
        let mut input = None;
        let mut output = None;
        let mut ambiguous = false;
        for endpoint in setting.endpoints.iter().filter(|endpoint| endpoint.bulk) {
            let target = if endpoint.address & 0x80 != 0 {
                &mut input
            } else {
                &mut output
            };
            ambiguous |= target.replace(endpoint).is_some();
        }
        if let (Some(input), Some(output)) = (input, output)
            && !ambiguous
            && input.packet_size > 0
            && output.packet_size > 0
        {
            return Ok([
                i32::from(device.configuration),
                i32::from(setting.number),
                i32::from(setting.alternate),
                i32::from(input.address),
                i32::from(output.address),
                i32::from(input.packet_size) * 16,
                i32::from(output.packet_size) * 16,
                i32::from(device.usb_version),
            ]);
        }
    }
    Err(Error::Usb(
        "Scope interface 0 does not have one bulk input and one bulk output".into(),
    ))
}

#[cfg(test)]
mod tests;
