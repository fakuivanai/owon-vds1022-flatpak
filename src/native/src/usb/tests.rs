//! Domain tests use disposable non-USB FDs and the production grant controller.
//! The portal's actual D-Bus acquisition validation has separate private-bus tests.

use super::*;
use std::fs::File;
use std::sync::{Arc, Mutex, Weak};

#[derive(Clone)]
struct FakeDevice {
    properties: UsbDevice,
    description: DeviceDescription,
    acquisition_error: Option<Error>,
    wrap_error: Option<TransportError>,
    claim_error: Option<TransportError>,
    release_error: Option<TransportError>,
    reset_error: Option<TransportError>,
    transfer_error: Option<TransportError>,
    grant_error: bool,
    transferred: usize,
}

struct State {
    devices: BTreeMap<String, FakeDevice>,
    events: Vec<String>,
    descriptors: Vec<(String, Weak<OwnedFd>)>,
    enumeration_error: bool,
    initialized: bool,
}

type Shared = Arc<Mutex<State>>;
struct MockPortal(Shared);
struct MockUsb(Shared);
struct MockHandle {
    id: String,
    shared: Shared,
    _fd: Arc<OwnedFd>,
}

impl Drop for MockHandle {
    fn drop(&mut self) {
        self.shared
            .lock()
            .unwrap()
            .events
            .push(format!("close:{}", self.id));
    }
}

impl PortalBackend for MockPortal {
    fn enumerate(&mut self) -> Result<Vec<UsbDevice>, Error> {
        let mut state = self.0.lock().unwrap();
        state.events.push("enumerate".into());
        if state.enumeration_error {
            Err(Error::Portal("enumeration failed".into()))
        } else {
            Ok(state
                .devices
                .values()
                .map(|device| device.properties.clone())
                .collect())
        }
    }

    fn acquire(&mut self, id: &str) -> Result<OwnedFd, Error> {
        let mut state = self.0.lock().unwrap();
        state.events.push(format!("acquire:{id}"));
        if let Some(error) = state.devices[id].acquisition_error.clone() {
            return Err(error);
        }
        Ok(File::open("/dev/null").unwrap().into())
    }

    fn release(&mut self, id: &str) -> Result<(), Error> {
        let mut state = self.0.lock().unwrap();
        // Production cleanup must destroy this USB handle and FD before portal release.
        assert!(
            state
                .descriptors
                .iter()
                .filter(|(device_id, _)| device_id == id)
                .all(|(_, fd)| fd.upgrade().is_none())
        );
        state.events.push(format!("grant-release:{id}"));
        if state
            .devices
            .get(id)
            .is_some_and(|device| device.grant_error)
        {
            Err(Error::Portal("release reply lost".into()))
        } else {
            Ok(())
        }
    }
}

impl UsbBackend for MockUsb {
    type Handle = MockHandle;

    fn init(&mut self) -> Result<(), TransportError> {
        let mut state = self.0.lock().unwrap();
        if !state.initialized {
            state.events.push("init-no-discovery".into());
            state.initialized = true;
        }
        Ok(())
    }

    fn wrap(&mut self, grant: &PortalGrant) -> Result<MockHandle, TransportError> {
        let mut state = self.0.lock().unwrap();
        state.events.push(format!("wrap:{}", grant.id));
        state
            .descriptors
            .push((grant.id.clone(), Arc::downgrade(&grant.fd)));
        if let Some(error) = state.devices[&grant.id].wrap_error.clone() {
            return Err(error);
        }
        Ok(MockHandle {
            id: grant.id.clone(),
            shared: Arc::clone(&self.0),
            _fd: Arc::clone(&grant.fd),
        })
    }

    fn describe(&mut self, handle: &MockHandle) -> Result<DeviceDescription, TransportError> {
        Ok(self.0.lock().unwrap().devices[&handle.id]
            .description
            .clone())
    }

    fn claim(&mut self, handle: &MockHandle) -> Result<(), TransportError> {
        let mut state = self.0.lock().unwrap();
        state.events.push(format!("claim:{}", handle.id));
        state.devices[&handle.id]
            .claim_error
            .clone()
            .map_or(Ok(()), Err)
    }

    fn release_interface(&mut self, handle: &MockHandle) -> Result<(), TransportError> {
        let mut state = self.0.lock().unwrap();
        state
            .events
            .push(format!("interface-release:{}", handle.id));
        state.devices[&handle.id]
            .release_error
            .clone()
            .map_or(Ok(()), Err)
    }

    fn reset(&mut self, handle: &MockHandle) -> Result<(), TransportError> {
        let mut state = self.0.lock().unwrap();
        state.events.push(format!("reset:{}", handle.id));
        state.devices[&handle.id]
            .reset_error
            .clone()
            .map_or(Ok(()), Err)
    }

    fn transfer(
        &mut self,
        handle: &MockHandle,
        endpoint: u8,
        buffer: &mut [u8],
        timeout: Duration,
        read: bool,
    ) -> Result<usize, TransportError> {
        assert!(timeout > Duration::ZERO);
        assert_eq!(endpoint, if read { 0x81 } else { 0x02 });
        let mut state = self.0.lock().unwrap();
        state.events.push(format!("transfer:{}", handle.id));
        let device = &state.devices[&handle.id];
        if let Some(error) = device.transfer_error.clone() {
            return Err(error);
        }
        let length = device.transferred.min(buffer.len());
        if read {
            buffer[..length].fill(0x55);
        }
        Ok(length)
    }

    fn shutdown(&mut self) {
        let mut state = self.0.lock().unwrap();
        if state.initialized {
            state.events.push("context-exit".into());
            state.initialized = false;
        }
    }
}

fn description() -> DeviceDescription {
    DeviceDescription {
        vendor: VENDOR_ID,
        product: PRODUCT_ID,
        configuration: 1,
        usb_version: 0x0200,
        interfaces: vec![InterfaceDescription {
            number: 0,
            alternate: 0,
            endpoints: vec![
                EndpointDescription {
                    address: 0x81,
                    bulk: true,
                    packet_size: 512,
                },
                EndpointDescription {
                    address: 0x02,
                    bulk: true,
                    packet_size: 512,
                },
            ],
        }],
    }
}

fn fake_device(id: &str) -> FakeDevice {
    FakeDevice {
        properties: UsbDevice {
            id: id.into(),
            readable: true,
            writable: true,
            vendor_id: Some("5345".into()),
            product_id: Some("1234".into()),
        },
        description: description(),
        acquisition_error: None,
        wrap_error: None,
        claim_error: None,
        release_error: None,
        reset_error: None,
        transfer_error: None,
        grant_error: false,
        transferred: 4,
    }
}

fn fixture() -> (Controller<MockPortal, MockUsb>, Shared) {
    let state = Arc::new(Mutex::new(State {
        devices: ["a", "b", "c"]
            .into_iter()
            .map(|id| (id.into(), fake_device(id)))
            .collect(),
        events: vec![],
        descriptors: vec![],
        enumeration_error: false,
        initialized: false,
    }));
    (
        Controller::with_backends(MockPortal(Arc::clone(&state)), MockUsb(Arc::clone(&state))),
        state,
    )
}

fn count(state: &Shared, event: &str) -> usize {
    state
        .lock()
        .unwrap()
        .events
        .iter()
        .filter(|value| value.as_str() == event)
        .count()
}

fn assert_closed(state: &Shared) {
    assert!(
        state
            .lock()
            .unwrap()
            .descriptors
            .iter()
            .all(|(_, fd)| fd.upgrade().is_none())
    );
}

#[test]
fn property_filter_requires_exact_ids_and_read_write() {
    let mut device = fake_device("a").properties;
    assert!(scope_properties(&device));
    for vendor in [None, Some("5344"), Some("0x5345"), Some("5345 ")] {
        device.vendor_id = vendor.map(str::to_owned);
        assert!(!scope_properties(&device));
    }
    device.vendor_id = Some("5345".into());
    for product in [None, Some("1235"), Some("1022")] {
        device.product_id = product.map(str::to_owned);
        assert!(!scope_properties(&device));
    }
    device.product_id = Some("1234".into());
    device.readable = false;
    assert!(!scope_properties(&device));
    device.readable = true;
    device.writable = false;
    assert!(!scope_properties(&device));
}

#[test]
fn descriptor_validation_rejects_ambiguous_or_wrong_interfaces() {
    let valid = description();
    assert_eq!(
        validate_description(&valid).unwrap(),
        [1, 0, 0, 0x81, 2, 8192, 8192, 0x0200]
    );
    let mut changed = valid.clone();
    changed.vendor = 1;
    assert!(validate_description(&changed).is_err());
    changed = valid.clone();
    changed.product = 1;
    assert!(validate_description(&changed).is_err());
    changed = valid.clone();
    changed.interfaces[0].number = 1;
    assert!(validate_description(&changed).is_err());
    changed = valid.clone();
    changed.interfaces[0].alternate = 1;
    assert!(validate_description(&changed).is_err());
    changed = valid.clone();
    let duplicate = changed.interfaces[0].endpoints[0].clone();
    changed.interfaces[0].endpoints.push(duplicate);
    assert!(validate_description(&changed).is_err());
    changed = valid.clone();
    changed.interfaces[0].endpoints[0].packet_size = 0;
    assert!(validate_description(&changed).is_err());
    changed = valid;
    changed.interfaces[0].endpoints[1].bulk = false;
    assert!(validate_description(&changed).is_err());
}

#[test]
fn probe_reopen_retains_exact_fd_and_uses_fresh_token() {
    let (mut controller, state) = fixture();
    let (old, _) = controller.open("a").unwrap();
    let fd = controller.active[&old].grant.fd.as_raw_fd();
    controller.close_probe(old).unwrap();
    assert!(controller.close(old).is_err());
    assert_eq!(controller.pending["a"].fd.as_raw_fd(), fd);
    let (new, _) = controller.open("a").unwrap();
    assert!(new > old);
    assert_eq!(controller.active[&new].grant.fd.as_raw_fd(), fd);
    assert_eq!(count(&state, "acquire:a"), 1);
    assert_eq!(count(&state, "wrap:a"), 2);
    controller.close(new).unwrap();
    assert_closed(&state);
    assert_eq!(count(&state, "grant-release:a"), 1);
}

#[test]
fn pending_grants_are_isolated_by_id() {
    let (mut controller, state) = fixture();
    let (a, _) = controller.open("a").unwrap();
    controller.close_probe(a).unwrap();
    let (b, _) = controller.open("b").unwrap();
    controller.close(b).unwrap();
    assert!(controller.pending.contains_key("a"));
    assert_eq!(count(&state, "grant-release:a"), 0);
    controller.forget("a").unwrap();
    assert_closed(&state);
}

#[test]
fn duplicate_open_preserves_the_active_grant() {
    let (mut controller, state) = fixture();
    let (token, _) = controller.open("a").unwrap();
    assert!(controller.open("a").is_err());
    assert_eq!(count(&state, "acquire:a"), 1);
    assert_eq!(count(&state, "grant-release:a"), 0);
    controller.close(token).unwrap();
}

#[test]
fn ineligible_device_is_never_acquired() {
    let (mut controller, state) = fixture();
    state
        .lock()
        .unwrap()
        .devices
        .get_mut("a")
        .unwrap()
        .properties
        .writable = false;
    assert!(controller.open("a").is_err());
    assert_eq!(count(&state, "acquire:a"), 0);
}

#[test]
fn rejected_probe_closes_and_releases_immediately() {
    let (mut controller, state) = fixture();
    let (token, _) = controller.open("a").unwrap();
    controller.close(token).unwrap();
    assert!(controller.pending.is_empty());
    assert_closed(&state);
}

#[test]
fn cancellation_remains_a_distinct_error() {
    let (mut controller, state) = fixture();
    state
        .lock()
        .unwrap()
        .devices
        .get_mut("a")
        .unwrap()
        .acquisition_error = Some(Error::Cancelled);
    assert_eq!(controller.open("a").unwrap_err(), Error::Cancelled);
    assert!(controller.active.is_empty() && controller.pending.is_empty());
    assert_eq!(count(&state, "wrap:a"), 0);
}

fn claim_failure(reused: bool) {
    let (mut controller, state) = fixture();
    if reused {
        let (token, _) = controller.open("a").unwrap();
        controller.close_probe(token).unwrap();
    }
    state
        .lock()
        .unwrap()
        .devices
        .get_mut("a")
        .unwrap()
        .claim_error = Some(TransportError::Failed("claim failed".into()));
    assert!(controller.open("a").is_err());
    assert_closed(&state);
    assert_eq!(count(&state, "acquire:a"), 1);
    assert_eq!(count(&state, "grant-release:a"), 1);
}

#[test]
fn initial_claim_failure_releases_grant() {
    claim_failure(false);
}
#[test]
fn reused_claim_failure_releases_grant() {
    claim_failure(true);
}

#[test]
fn failed_probe_release_does_not_retain_grant() {
    let (mut controller, state) = fixture();
    let (token, _) = controller.open("a").unwrap();
    state
        .lock()
        .unwrap()
        .devices
        .get_mut("a")
        .unwrap()
        .release_error = Some(TransportError::NoDevice);
    assert!(controller.close_probe(token).is_err());
    assert!(controller.pending.is_empty());
    assert_closed(&state);
    assert_eq!(count(&state, "grant-release:a"), 1);
}

#[test]
fn unplugged_pending_grant_is_consumed_and_released() {
    let (mut controller, state) = fixture();
    let (token, _) = controller.open("a").unwrap();
    controller.close_probe(token).unwrap();
    state.lock().unwrap().devices.remove("a");
    assert!(controller.open("a").is_err());
    assert_closed(&state);
    assert_eq!(count(&state, "acquire:a"), 1);
}

#[test]
fn descriptor_change_after_probe_fails_closed() {
    let (mut controller, state) = fixture();
    let (token, _) = controller.open("a").unwrap();
    controller.close_probe(token).unwrap();
    state
        .lock()
        .unwrap()
        .devices
        .get_mut("a")
        .unwrap()
        .description
        .product = 0x9999;
    assert!(controller.open("a").is_err());
    assert_closed(&state);
    assert_eq!(count(&state, "grant-release:a"), 1);
}

#[test]
fn failed_wrap_drops_initial_and_retained_descriptors_before_release() {
    for retained in [false, true] {
        let (mut controller, state) = fixture();
        if retained {
            let (token, _) = controller.open("a").unwrap();
            controller.close_probe(token).unwrap();
        }
        state
            .lock()
            .unwrap()
            .devices
            .get_mut("a")
            .unwrap()
            .wrap_error = Some(TransportError::NoDevice);
        assert!(controller.open("a").is_err());
        assert!(controller.pending.is_empty());
        assert!(controller.active.is_empty());
        assert_closed(&state);
        assert_eq!(count(&state, "acquire:a"), 1);
        assert_eq!(count(&state, "grant-release:a"), 1);
    }
}

#[test]
fn failed_portal_acquisition_never_wraps_a_descriptor() {
    let (mut controller, state) = fixture();
    for message in ["mismatched finish ID", "invalid FD result"] {
        state
            .lock()
            .unwrap()
            .devices
            .get_mut("a")
            .unwrap()
            .acquisition_error = Some(Error::Portal(message.into()));
        assert!(controller.open("a").is_err());
        assert_eq!(count(&state, "wrap:a"), 0);
    }
}

#[test]
fn shutdown_cleans_active_and_pending_even_when_release_fails() {
    let (mut controller, state) = fixture();
    let (a, _) = controller.open("a").unwrap();
    controller.close_probe(a).unwrap();
    controller.open("b").unwrap();
    state
        .lock()
        .unwrap()
        .devices
        .get_mut("a")
        .unwrap()
        .grant_error = true;
    assert!(controller.shutdown().is_err());
    assert_closed(&state);
    assert_eq!(count(&state, "grant-release:a"), 1);
    assert_eq!(count(&state, "grant-release:b"), 1);
    assert_eq!(count(&state, "context-exit"), 1);
}

#[test]
fn enumeration_failure_preserves_original_error_and_active_handle() {
    let (mut controller, state) = fixture();
    let (a, _) = controller.open("a").unwrap();
    controller.close_probe(a).unwrap();
    let (b, _) = controller.open("b").unwrap();
    {
        let mut state = state.lock().unwrap();
        state.enumeration_error = true;
        state.devices.get_mut("a").unwrap().grant_error = true;
    }
    assert_eq!(
        controller.enumerate().unwrap_err(),
        Error::Portal("enumeration failed".into())
    );
    assert!(controller.pending.is_empty());
    let mut buffer = [0; 4];
    assert_eq!(controller.transfer(b, &mut buffer, 1, true).unwrap(), 4);
    controller.close(b).unwrap();
    assert_closed(&state);
}

#[test]
fn reset_closes_handle_even_when_device_has_disappeared() {
    let (mut controller, state) = fixture();
    let (token, _) = controller.open("a").unwrap();
    state
        .lock()
        .unwrap()
        .devices
        .get_mut("a")
        .unwrap()
        .reset_error = Some(TransportError::NoDevice);
    controller.reset(token).unwrap();
    assert!(controller.close(token).is_err());
    assert_closed(&state);
}

#[test]
fn transfer_validation_partial_progress_and_disconnection() {
    let (mut controller, state) = fixture();
    let (token, _) = controller.open("a").unwrap();
    let mut buffer = [0; 8];
    assert!(matches!(
        controller.transfer(token, &mut buffer, 0, true),
        Err(Error::InvalidArgument(_))
    ));
    assert_eq!(controller.transfer(token, &mut buffer, 1, true).unwrap(), 4);
    assert_eq!(buffer, [0x55, 0x55, 0x55, 0x55, 0, 0, 0, 0]);
    assert_eq!(
        controller.transfer(token, &mut buffer, 1, false).unwrap(),
        4
    );
    state
        .lock()
        .unwrap()
        .devices
        .get_mut("a")
        .unwrap()
        .transfer_error = Some(TransportError::NoDevice);
    assert!(controller.transfer(token, &mut buffer, 1, true).is_err());
    assert!(controller.active.contains_key(&token));
    assert_eq!(count(&state, "grant-release:a"), 0);
    state
        .lock()
        .unwrap()
        .devices
        .get_mut("a")
        .unwrap()
        .release_error = Some(TransportError::NoDevice);
    controller.close(token).unwrap();
    assert!(controller.close(token).is_err());
    assert_closed(&state);
}

#[test]
fn tokens_are_never_reused_after_shutdown_and_exhaustion_cleans_up() {
    let (mut controller, state) = fixture();
    let (first, _) = controller.open("a").unwrap();
    controller.shutdown().unwrap();
    let (next, _) = controller.open("a").unwrap();
    assert!(next > first);
    controller.close(next).unwrap();
    controller.next_token = i64::MAX as u64 + 1;
    assert!(controller.open("a").is_err());
    assert_closed(&state);
    assert!(controller.active.is_empty());
}

#[test]
fn drop_performs_shutdown_cleanup() {
    let (mut controller, state) = fixture();
    let (token, _) = controller.open("a").unwrap();
    controller.close_probe(token).unwrap();
    drop(controller);
    assert_closed(&state);
    assert_eq!(count(&state, "grant-release:a"), 1);
}
